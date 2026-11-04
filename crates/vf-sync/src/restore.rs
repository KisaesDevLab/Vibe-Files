//! Restore (P11 step 8, done-when 4): a bare PC with the bucket's credentials and the sync
//! passphrase rebuilds the cabinet: `cabinet.json` → key → HEAD → segments → folders with their
//! sentinels (exact text), template folders, documents (SHA-256 checked), versions, docmeta,
//! master files and the audit trail's monthly files. Also: conflicts (step 6) and removing
//! disposed objects once every workstation has seen the tombstone (step 7).

use std::path::Path;
use std::sync::Arc;

use object_store::ObjectStore;
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Transaction};

use crate::layout::{self, CabinetSegment, ClientSegment, DeviceRecord};
use crate::remote::Remote;
use crate::{Result, Sync, SyncError};

/// Cabinet ids in the bucket.
pub fn cabinets(store: Arc<dyn ObjectStore>) -> Result<Vec<String>> {
    let all = Remote::new(store, "").list_root("vf/")?;
    let mut out: Vec<String> = all
        .iter()
        .filter_map(|k| k.strip_prefix("vf/")?.strip_suffix("/cabinet.json"))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreReport {
    pub clients: usize,
    pub documents: usize,
    pub audit_months: usize,
}

/// Restores cabinet `cabinet_id` into `root` (missing or empty) with a new catalog `conn`.
pub fn restore(
    conn: &mut Connection,
    root: &Path,
    store: Arc<dyn ObjectStore>,
    cabinet_id: &str,
    passphrase: &str,
    device_id: &str,
    device_name: &str,
) -> Result<RestoreReport> {
    if root.exists()
        && std::fs::read_dir(root)
            .map_err(|e| SyncError::Local(e.to_string()))?
            .next()
            .is_some()
    {
        return Err(SyncError::Local(format!(
            "{} is not empty: restore into a new folder",
            root.display()
        )));
    }
    let remote = Remote::new(store, cabinet_id);
    let b = crate::setup::bucket_cabinet(&remote)?.ok_or(SyncError::NotSetUp)?;
    let w = b
        .wrapped
        .get(crate::setup::KEY_ID)
        .ok_or(SyncError::NotSetUp)?;
    let firm = vf_security::keys::unwrap_with_passphrase(w, passphrase, cabinet_id)
        .map_err(|_| SyncError::WrongPassphrase)?;
    let keys = crate::Keys::derive(crate::setup::KEY_ID, &firm);
    let s = Sync {
        remote,
        keys,
        cabinet_id: cabinet_id.into(),
        device_id: device_id.into(),
        device_name: device_name.into(),
    };
    restore_session(conn, root, &s)
}

/// [`restore`] with an unlocked session (Sync tab › Restore cabinet to a folder).
pub fn restore_session(conn: &mut Connection, root: &Path, s: &Sync) -> Result<RestoreReport> {
    if root.exists()
        && std::fs::read_dir(root)
            .map_err(|e| SyncError::Local(e.to_string()))?
            .next()
            .is_some()
    {
        return Err(SyncError::Local(format!(
            "{} is not empty: restore into a new folder",
            root.display()
        )));
    }
    let device_id = s.device_id.as_str();
    let b = crate::setup::bucket_cabinet(&s.remote)?.ok_or(SyncError::NotSetUp)?;
    let w = b
        .wrapped
        .get(crate::setup::KEY_ID)
        .ok_or(SyncError::NotSetUp)?;
    let vibe = root.join(vf_fs::cabinet::VIBE_DIR);
    std::fs::create_dir_all(&vibe).map_err(|e| SyncError::Local(e.to_string()))?;
    let head = s.head()?.ok_or(SyncError::NotSetUp)?;

    // The cabinet file first: everything else reads it.
    let cab_seg: CabinetSegment = s
        .get_json(&head.cabinet_key())?
        .ok_or_else(|| SyncError::Format("the cabinet segment is missing".into()))?;
    cab_seg
        .check()
        .map_err(|what| SyncError::Format(format!("the cabinet segment: refused {what}")))?;
    let cab_text = cab_seg
        .files
        .get("cabinet.json")
        .cloned()
        .ok_or_else(|| SyncError::Format("the cabinet segment has no cabinet.json".into()))?;
    vf_fs::cabinet::write_new(&vibe.join("cabinet.json"), cab_text.as_bytes())?;
    std::fs::create_dir_all(vibe.join(vf_fs::cabinet::KEYS_DIR))
        .map_err(|e| SyncError::Local(e.to_string()))?;
    vf_fs::cabinet::write_replace(&crate::setup::key_file(root), &w.to_json())?;

    let mut rep = crate::pull::PullReport::default();
    crate::pull::apply_head(conn, root, s, &head, &mut rep)?;
    crate::catalog::set_state(conn, device_id, head.rev, "pull")?;

    let mut out = RestoreReport {
        clients: head.segments.len(),
        documents: rep.new,
        audit_months: 0,
    };
    let dir = vibe.join("audit");
    for key in s.remote.list("audit/")? {
        let Some(month) = key
            .strip_prefix("audit/")
            .filter(|m| crate::layout::safe_month(m))
        else {
            continue;
        };
        let Some(body) = s.remote.get(&key)? else {
            continue;
        };
        let plain = s.keys.open(&body, &key)?;
        std::fs::create_dir_all(&dir).map_err(|e| SyncError::Local(e.to_string()))?;
        std::fs::write(dir.join(format!("{month}.jsonl")), plain)
            .map_err(|e| SyncError::Local(e.to_string()))?;
        out.audit_months += 1;
    }
    // Rows the folders imply (the cabinet row, versions from sidecars).
    vf_fs::rebuild::rebuild(conn, root, device_id).map_err(|e| e.to_string())?;
    crate::push::write_device(s, head.rev)?;
    Transaction::new("sync_restore")
        .audit(AuditEntry::new(
            "sync_restore",
            None,
            json!({ "clients": out.clients, "documents": out.documents, "rev": head.rev }),
        ))
        .commit(conn, &mut NoFileOps, &Actor::device(device_id))?;
    tracing::info!(
        clients = out.clients,
        documents = out.documents,
        "sync restore"
    );
    Ok(out)
}

/// An open Conflicts item.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub id: String,
    pub doc_id: String,
    pub copy_doc_id: Option<String>,
    pub kind: String,
    pub from_device: String,
    pub at: String,
    pub rel_path: String,
    pub copy_rel_path: Option<String>,
}

pub fn conflicts(conn: &Connection) -> Result<Vec<Conflict>> {
    let mut st = conn.prepare(
        "SELECT c.id, c.doc_id, c.copy_doc_id, c.kind, c.from_device, c.at, d.rel_path, e.rel_path
         FROM sync_conflict c JOIN document d ON d.id = c.doc_id
         LEFT JOIN document e ON e.id = c.copy_doc_id
         WHERE c.resolved IS NULL ORDER BY c.at",
    )?;
    let v = st
        .query_map([], |r| {
            Ok(Conflict {
                id: r.get(0)?,
                doc_id: r.get(1)?,
                copy_doc_id: r.get(2)?,
                kind: r.get(3)?,
                from_device: r.get(4)?,
                at: r.get(5)?,
                rel_path: r.get(6)?,
                copy_rel_path: r.get(7)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// This workstation's content becomes the document's (a new version); the copy goes to the
    /// trash.
    Mine,
    /// The other workstation's content stays; the copy goes to the trash.
    Theirs,
    /// Both stay as they are.
    Both,
}

/// Resolves a Conflicts item. Changes go up on the next push.
pub fn resolve(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    id: &str,
    keep: Keep,
) -> Result<()> {
    let c = conflicts(conn)?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| SyncError::Local("that conflict is resolved already".into()))?;
    if let (Some(copy), Some(copy_rel)) = (&c.copy_doc_id, &c.copy_rel_path) {
        match keep {
            Keep::Mine => {
                let tmp = vf_fs::content::tmp_file(root, "part")?;
                std::fs::copy(root.join(copy_rel), &tmp)
                    .map_err(|e| SyncError::Local(e.to_string()))?;
                match vf_fs::content::replace(
                    conn,
                    root,
                    device_id,
                    &c.doc_id,
                    &tmp,
                    vf_fs::content::Change {
                        label: "sync_conflict_keep_mine",
                        created_by: "sync",
                        reason: "the other workstation's version, replaced in a conflict",
                        detail: json!({}),
                        extra: vec![],
                        based_on: None,
                    },
                )? {
                    Ok(_) => {}
                    Err(why) => return Err(SyncError::Local(why.to_string())),
                }
                vf_fs::trash::delete(
                    conn,
                    root,
                    device_id,
                    std::slice::from_ref(copy),
                    "conflict resolved",
                )?;
            }
            Keep::Theirs => {
                vf_fs::trash::delete(
                    conn,
                    root,
                    device_id,
                    std::slice::from_ref(copy),
                    "conflict resolved",
                )?;
            }
            Keep::Both => {}
        }
    }
    let how = match keep {
        Keep::Mine => "mine",
        Keep::Theirs => "theirs",
        Keep::Both => "both",
    };
    conn.execute(
        "UPDATE sync_conflict SET resolved = ?2, resolved_at = ?3 WHERE id = ?1",
        [id, how, &vf_core::now_rfc3339()],
    )?;
    Transaction::new("sync_conflict_resolved")
        .audit(AuditEntry::new(
            "sync_conflict_resolved",
            Some(&c.doc_id),
            json!({ "keep": how }),
        ))
        .commit(conn, &mut NoFileOps, &Actor::device(device_id))?;
    Ok(())
}

/// Removes the objects of disposed documents (P07) from the bucket once every workstation has
/// applied the manifest that carries their tombstone, and no live document or version uses the
/// same content. Returns how many objects were removed.
pub fn remove_disposed(s: &Sync) -> Result<usize> {
    remove_disposed_in(s, None)
}

/// [`remove_disposed`]; with a catalog, its "already in the bucket" note is dropped for each
/// object removed.
pub fn remove_disposed_in(s: &Sync, conn: Option<&Connection>) -> Result<usize> {
    let Some(head) = s.head()? else {
        return Ok(0);
    };
    let mut acked = u64::MAX;
    for key in s.remote.list("devices/")? {
        if let Some(d) = s.get_json::<DeviceRecord>(&key)? {
            acked = acked.min(d.acked_rev);
        }
    }
    if acked < head.rev {
        return Ok(0); // a workstation has not seen the latest manifest yet
    }
    let mut live = std::collections::BTreeSet::new();
    let mut gone = std::collections::BTreeSet::new();
    let mut docs = vec![];
    if head.cabinet_segment_rev > 0
        && let Some(c) = s.get_json::<CabinetSegment>(&head.cabinet_key())?
    {
        docs.extend(c.documents);
    }
    for (client, rev) in &head.segments {
        if let Some(c) = s.get_json::<ClientSegment>(&head.client_key(client, *rev))? {
            docs.extend(c.documents);
        }
    }
    for d in &docs {
        let disposed = d.deleted_at.is_some() && d.tombstone_reason.as_deref() == Some("disposed");
        for o in std::iter::once(&d.object).chain(d.versions.iter().map(|v| &v.object)) {
            if disposed {
                gone.insert(o.clone());
            } else {
                live.insert(o.clone());
            }
        }
    }
    let mut n = 0;
    let hash_of: std::collections::BTreeMap<&String, &String> = docs
        .iter()
        .flat_map(|d| {
            std::iter::once((&d.object, &d.hash))
                .chain(d.versions.iter().map(|v| (&v.object, &v.hash)))
        })
        .collect();
    for o in gone.difference(&live) {
        let key = layout::object_key(o);
        if s.remote.exists(&key)? {
            s.remote.delete(&key)?;
            n += 1;
        }
        if let (Some(c), Some(h)) = (conn, hash_of.get(o)) {
            c.execute("DELETE FROM sync_object WHERE hash = ?1", [h])?;
        }
    }
    Ok(n)
}

/// Every live document in the current manifest, with its folder (cabinet-relative path, entry).
fn current_docs(s: &Sync) -> Result<Vec<(String, crate::layout::DocEntry)>> {
    let Some(head) = s.head()? else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    if head.cabinet_segment_rev > 0
        && let Some(c) = s.get_json::<CabinetSegment>(&head.cabinet_key())?
    {
        out.extend(c.documents.into_iter().map(|d| (String::new(), d)));
    }
    for (client, rev) in &head.segments {
        if let Some(c) = s.get_json::<ClientSegment>(&head.client_key(client, *rev))? {
            let f = c.folder_rel.clone();
            out.extend(c.documents.into_iter().map(|d| (f.clone(), d)));
        }
    }
    out.retain(|(_, d)| d.deleted_at.is_none());
    Ok(out)
}

/// Verify now (P11 step 8): downloads up to `sample` documents spread over the manifest and
/// checks each against its SHA-256. Returns (checked, failed).
pub fn verify(s: &Sync, sample: usize) -> Result<(usize, usize)> {
    let docs = current_docs(s)?;
    if docs.is_empty() || sample == 0 {
        return Ok((0, 0));
    }
    let step = docs.len().div_ceil(sample).max(1);
    let mut start = [0u8; 2];
    getrandom::fill(&mut start).map_err(|e| SyncError::Local(e.to_string()))?;
    let offset = u16::from_le_bytes(start) as usize % step;
    let (mut checked, mut bad) = (0, 0);
    for (_, d) in docs.iter().skip(offset).step_by(step).take(sample) {
        let key = layout::object_key(&d.object);
        checked += 1;
        let ok = s
            .remote
            .get(&key)?
            .and_then(|b| s.keys.open(&b, &key).ok())
            .is_some_and(|p| crate::crypto::sha256(&p) == d.hash);
        if !ok {
            bad += 1;
        }
    }
    Ok((checked, bad))
}

/// Restore one client to a folder (P11 step 8): its current documents, as plain files under
/// `dest/<client folder>/`, each checked against its SHA-256. Returns how many.
pub fn export_client(s: &Sync, client_id: &str, dest: &Path) -> Result<usize> {
    let head = s.head()?.ok_or(SyncError::NotSetUp)?;
    let rev = head
        .segments
        .get(client_id)
        .ok_or_else(|| SyncError::Local("this client is not in the bucket".into()))?;
    let seg: ClientSegment = s
        .get_json(&head.client_key(client_id, *rev))?
        .ok_or_else(|| SyncError::Format("the client segment is missing".into()))?;
    seg.check()
        .map_err(|what| SyncError::Format(format!("the client segment: refused {what}")))?;
    let base = dest.join(&seg.folder_rel);
    let mut n = 0;
    for d in seg.documents.iter().filter(|d| d.deleted_at.is_none()) {
        let key = layout::object_key(&d.object);
        let body = s
            .remote
            .get(&key)?
            .ok_or_else(|| SyncError::Format(format!("{key} is missing")))?;
        let plain = s.keys.open(&body, &key)?;
        if crate::crypto::sha256(&plain) != d.hash {
            return Err(SyncError::Format(format!(
                "{key} does not match its SHA-256"
            )));
        }
        let p = filer_intake::commit::free_path(&base.join(&d.rel_path));
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SyncError::Local(e.to_string()))?;
        }
        std::fs::write(&p, plain).map_err(|e| SyncError::Local(e.to_string()))?;
        n += 1;
    }
    Ok(n)
}
