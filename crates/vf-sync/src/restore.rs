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
    let vibe = root.join(vf_fs::cabinet::VIBE_DIR);
    std::fs::create_dir_all(&vibe).map_err(|e| SyncError::Local(e.to_string()))?;
    let s = Sync {
        remote,
        keys,
        cabinet_id: cabinet_id.into(),
        device_id: device_id.into(),
        device_name: device_name.into(),
    };
    let head = s.head()?.ok_or(SyncError::NotSetUp)?;

    // The cabinet file first: everything else reads it.
    let cab_seg: CabinetSegment = s
        .get_json(&layout::cabinet_segment_key(head.cabinet_segment_rev))?
        .ok_or_else(|| SyncError::Format("the cabinet segment is missing".into()))?;
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
    crate::pull::apply_head(conn, root, &s, &head, &mut rep)?;
    crate::catalog::set_state(conn, device_id, head.rev, "pull")?;

    let mut out = RestoreReport {
        clients: head.segments.len(),
        documents: rep.new,
        audit_months: 0,
    };
    let dir = vibe.join("audit");
    for key in s.remote.list("audit/")? {
        let Some(month) = key.strip_prefix("audit/") else {
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
    crate::push::write_device(&s, head.rev)?;
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
        && let Some(c) =
            s.get_json::<CabinetSegment>(&layout::cabinet_segment_key(head.cabinet_segment_rev))?
    {
        docs.extend(c.documents);
    }
    for (client, rev) in &head.segments {
        if let Some(c) = s.get_json::<ClientSegment>(&layout::segment_key(client, *rev))? {
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
    for o in gone.difference(&live) {
        let key = layout::object_key(o);
        if s.remote.exists(&key)? {
            s.remote.delete(&key)?;
            n += 1;
        }
    }
    Ok(n)
}
