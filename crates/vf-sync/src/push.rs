//! Push (P11 step 3): changed documents' contents go up (content-addressed, skipped when the
//! bucket has them, so an interrupted push resumes), then each changed client's segment, then
//! the cabinet segment, then `manifest/HEAD`. B2 has no conditional PUT, so HEAD is written and
//! read back: when another workstation's HEAD is there instead, this push pulls and tries again.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Transaction};

use crate::catalog::{self, CABINET};
use crate::crypto::sha256;
use crate::layout::{self, CabinetSegment, ClientSegment, DeviceRecord, Head};
use crate::{Result, Sync, SyncError};

/// Transaction / audit label.
pub const LABEL: &str = "sync_push";
const TRIES: usize = 3;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PushReport {
    pub objects: usize,
    pub bytes: u64,
    pub segments: usize,
    pub documents: usize,
    /// Files that no longer match the catalog (edited outside Vibe Files): left for the
    /// integrity sweep, sent next time.
    pub skipped: usize,
    pub head_rev: u64,
    /// The pull this push ran first.
    pub pulled: Option<crate::pull::PullReport>,
}

/// The cabinet's master files (`_Vibe`-relative path → text), and parser profiles.
pub fn master_files(root: &Path) -> BTreeMap<String, String> {
    let vibe = root.join(vf_fs::cabinet::VIBE_DIR);
    let mut out = BTreeMap::new();
    // Taxpayer-ID columns are stripped from the client list on disk before it is sent.
    let _ = vf_fs::clients::strip_tins_on_disk(root);
    for f in layout::MASTER_FILES {
        if let Ok(t) = std::fs::read_to_string(vibe.join(f)) {
            out.insert((*f).to_string(), t);
        }
    }
    if let Ok(rd) = std::fs::read_dir(vibe.join("parsers")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".toml")
                && let Ok(t) = std::fs::read_to_string(e.path())
            {
                out.insert(format!("parsers/{n}"), t);
            }
        }
    }
    out
}

pub fn files_hashes(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(k, v)| (k.clone(), sha256(v.as_bytes())))
        .collect()
}

/// Sends what changed. `only` limits the push to one client (Browse's "Push this client now";
/// the cabinet's master files go with it when they changed).
pub fn push(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    only: Option<&str>,
) -> Result<PushReport> {
    let mut rep = PushReport::default();
    for attempt in 0..TRIES {
        // Up to date with the bucket first: a push never hides another workstation's work.
        let head = s.head()?.unwrap_or_default();
        if head.rev > catalog::last_rev(conn, &s.device_id)? {
            rep.pulled = Some(crate::pull::pull(conn, root, s)?);
        }
        match push_once(conn, root, s, only, &mut rep) {
            Err(SyncError::Raced) if attempt + 1 < TRIES => {
                tracing::info!(attempt, "sync push raced another workstation; retrying");
                continue;
            }
            r => return r.map(|()| rep),
        }
    }
    Err(SyncError::Raced)
}

fn push_once(
    conn: &mut Connection,
    root: &Path,
    s: &Sync,
    only: Option<&str>,
    rep: &mut PushReport,
) -> Result<()> {
    let head = s.head()?.unwrap_or_default();
    let writer = vf_core::new_id();
    let pending = catalog::pending(conn)?;
    let mut uploaded = catalog::uploaded(conn)?;
    // The admin password file deleted here after it was exchanged (the forgotten-password
    // reset): sent as removed, so the other places drop it too (owner 2026-11-04).
    let pin = vf_fs::trash::pin_file(root);
    if !pin.exists() && catalog::files_base(conn)?.contains_key("trash_pin.json") {
        vf_fs::cabinet::write_replace(&pin, vf_fs::trash::REMOVED_MARKER.as_bytes())
            .map_err(|e| SyncError::Local(e.to_string()))?;
    }
    let files = master_files(root);
    let hashes = files_hashes(&files);
    let files_dirty = hashes != catalog::files_base(conn)?;

    let mut new_head = head.clone();
    // What was sent, as sent (the live rows may change while the push runs): the base later.
    let mut sent_docs: Vec<SentDoc> = vec![];
    let mut sent_segments: Vec<(String, u64, Option<String>, Option<String>)> = vec![];

    // `Ok(false)`: the local file cannot be read or no longer has this content (edited outside
    // Vibe Files); bucket errors are errors.
    let mut upload = |hash: &str, path: &Path, rep: &mut PushReport| -> Result<bool> {
        if uploaded.contains(hash) {
            return Ok(true);
        }
        let key = layout::object_key(&s.keys.object_name(hash));
        if !s.remote.exists(&key)? {
            let Ok(bytes) = std::fs::read(path) else {
                return Ok(false);
            };
            if sha256(&bytes) != hash {
                return Ok(false);
            }
            rep.bytes += bytes.len() as u64;
            s.remote.put(&key, s.keys.seal(&bytes, &key))?;
            rep.objects += 1;
        }
        conn_note_uploaded(conn, hash)?;
        uploaded.insert(hash.to_string());
        Ok(true)
    };

    // A segment this PC has not applied (a pull left documents for later) is never sent: it
    // would put back what the other workstation changed.
    let applied = |c: &str| -> Result<bool> {
        let theirs = if c == CABINET {
            head.cabinet_segment_rev
        } else {
            head.segments.get(c).copied().unwrap_or(0)
        };
        Ok(theirs == catalog::segment_rev(conn, c)? || theirs == 0)
    };
    let clients: Vec<String> = pending
        .keys()
        .filter(|c| only.is_none_or(|o| o == c.as_str()))
        .cloned()
        .collect();
    let cabinet_too = (files_dirty || clients.iter().any(|c| c == CABINET)) && applied(CABINET)?;
    'clients: for client in clients.iter().filter(|c| c.as_str() != CABINET) {
        if !applied(client)? {
            rep.skipped += pending.get(client).copied().unwrap_or(0);
            continue;
        }
        let Some(folder) = catalog::client_folder(conn, client)? else {
            continue; // a client with no folder has no segment
        };
        let sentinel_path = filer_intake::sentinel::sentinel_path(&root.join(&folder));
        let Ok(sentinel) = std::fs::read_to_string(&sentinel_path) else {
            continue;
        };
        let mut docs = catalog::entries(conn, &s.keys, client, &folder)?;
        for e in &mut docs {
            if e.deleted_at.is_none()
                && !upload(&e.hash, &root.join(&folder).join(&e.rel_path), rep)?
            {
                // A file that cannot be read now: the client's segment waits (the bucket keeps
                // its previous one, nothing is dropped from it).
                rep.skipped += pending.get(client).copied().unwrap_or(1);
                continue 'clients;
            }
            let mut kept = Vec::with_capacity(e.versions.len());
            for v in std::mem::take(&mut e.versions) {
                if upload(&v.hash, &vf_fs::cabinet::version_path(root, &v.hash), rep)? {
                    kept.push(v);
                }
            }
            e.versions = kept;
        }
        for e in &docs {
            sent_docs.push(SentDoc::of(e, Some(client), &folder));
        }
        let rev = head.segments.get(client).copied().unwrap_or(0) + 1;
        let seg = ClientSegment {
            client_id: client.clone(),
            segment_rev: rev,
            folder_rel: folder.clone(),
            sentinel,
            documents: docs,
            empty_dirs: catalog::empty_dirs(root, &folder),
            written_by: s.device_name.clone(),
        };
        s.put_json(&layout::segment_key_by(client, rev, &writer), &seg)?;
        new_head.segments.insert(client.clone(), rev);
        new_head
            .segment_writers
            .insert(client.clone(), writer.clone());
        sent_segments.push((
            client.clone(),
            rev,
            Some(folder),
            Some(sha256(seg.sentinel.as_bytes())),
        ));
        rep.segments += 1;
    }
    if cabinet_too {
        let mut docs = catalog::entries(conn, &s.keys, CABINET, "")?;
        let mut ok = true;
        for e in &mut docs {
            if e.deleted_at.is_none() && !upload(&e.hash, &root.join(&e.rel_path), rep)? {
                ok = false;
                break;
            }
            let mut kept = Vec::with_capacity(e.versions.len());
            for v in std::mem::take(&mut e.versions) {
                if upload(&v.hash, &vf_fs::cabinet::version_path(root, &v.hash), rep)? {
                    kept.push(v);
                }
            }
            e.versions = kept;
        }
        if ok {
            for e in &docs {
                sent_docs.push(SentDoc::of(e, None, ""));
            }
            let rev = head.cabinet_segment_rev + 1;
            let seg = CabinetSegment {
                segment_rev: rev,
                files,
                documents: docs,
                written_by: s.device_name.clone(),
            };
            s.put_json(&layout::cabinet_segment_key_by(rev, &writer), &seg)?;
            new_head.cabinet_segment_rev = rev;
            new_head.cabinet_writer = writer.clone();
            sent_segments.push((CABINET.into(), rev, None, None));
            rep.segments += 1;
        } else {
            rep.skipped += 1;
        }
    }
    push_audit(conn, root, s)?;
    if rep.segments == 0 {
        rep.head_rev = head.rev;
        return Ok(());
    }

    // The commit point. Another push that committed while this one uploaded (its HEAD moved
    // on) wins: this one pulls and starts again, so neither change is lost.
    let now_there = s.head()?.unwrap_or_default();
    if now_there.rev != head.rev || now_there.writer != head.writer {
        return Err(SyncError::Raced);
    }
    new_head.rev = head.rev + 1;
    new_head.written_at = vf_core::now_rfc3339();
    new_head.device_id = s.device_id.clone();
    new_head.device_name = s.device_name.clone();
    new_head.writer = writer;
    s.put_json(layout::HEAD, &new_head)?;
    let back: Head = s.head()?.unwrap_or_default();
    if back.writer != new_head.writer {
        return Err(SyncError::Raced);
    }

    // What was sent is now the base.
    let tx = vf_core::db::begin_write(conn)?;
    for d in &sent_docs {
        catalog::set_base_values(
            &tx,
            &d.doc_id,
            d.client.as_deref(),
            &d.rel_path,
            &d.hash,
            d.rev,
            d.deleted,
        )?;
    }
    for (c, rev, folder, sh) in &sent_segments {
        let fh = (c == CABINET).then(|| serde_json::to_string(&hashes).expect("serializes"));
        catalog::set_segment(
            &tx,
            c,
            *rev,
            folder.as_deref(),
            sh.as_deref(),
            fh.as_deref(),
        )?;
    }
    for d in &sent_docs {
        tx.execute("DELETE FROM sync_pending WHERE doc_id = ?1", [&d.doc_id])?;
    }
    catalog::set_state(&tx, &s.device_id, new_head.rev, "push")?;
    tx.commit()?;
    rep.documents = sent_docs.len();
    rep.head_rev = new_head.rev;
    write_device(s, new_head.rev)?;
    Transaction::new(LABEL)
        .audit(AuditEntry::new(
            LABEL,
            None,
            json!({
                "objects": rep.objects, "bytes": rep.bytes, "segments": rep.segments,
                "documents": rep.documents, "rev": rep.head_rev, "skipped": rep.skipped,
            }),
        ))
        .commit(conn, &mut NoFileOps, &Actor::device(&s.device_id))?;
    tracing::info!(
        objects = rep.objects,
        segments = rep.segments,
        rev = rep.head_rev,
        "sync push"
    );
    Ok(())
}

/// A document as it went into a segment.
struct SentDoc {
    doc_id: String,
    client: Option<String>,
    /// Cabinet-relative.
    rel_path: String,
    hash: String,
    rev: i64,
    deleted: bool,
}

impl SentDoc {
    fn of(e: &crate::layout::DocEntry, client: Option<&str>, folder: &str) -> SentDoc {
        SentDoc {
            doc_id: e.doc_id.clone(),
            client: client.map(str::to_owned),
            rel_path: if folder.is_empty() {
                e.rel_path.clone()
            } else {
                format!("{folder}/{}", e.rel_path)
            },
            hash: e.hash.clone(),
            rev: e.rev,
            deleted: e.deleted_at.is_some(),
        }
    }
}

fn conn_note_uploaded(conn: &Connection, hash: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO sync_object (hash) VALUES (?1)",
        [hash],
    )?;
    Ok(())
}

pub(crate) fn write_device(s: &Sync, acked: u64) -> Result<()> {
    s.put_json(
        &layout::device_key(&s.device_id),
        &DeviceRecord {
            device_id: s.device_id.clone(),
            name: s.device_name.clone(),
            mode: "local".into(),
            last_seen: vf_core::now_rfc3339(),
            acked_rev: acked,
        },
    )
}

/// `_Vibe/audit/<month>.jsonl` files changed since last sent go to `audit/<month>`.
fn push_audit(conn: &Connection, root: &Path, s: &Sync) -> Result<()> {
    let dir = root.join(vf_fs::cabinet::VIBE_DIR).join("audit");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let Some(month) = n.strip_suffix(".jsonl") else {
            continue;
        };
        let Ok(bytes) = std::fs::read(e.path()) else {
            continue;
        };
        let h = sha256(&bytes);
        let k = format!("sync_audit:{month}");
        let sent: Option<String> = conn
            .query_row("SELECT v FROM local_state WHERE k = ?1", [&k], |r| r.get(0))
            .ok();
        if sent.as_deref() == Some(h.as_str()) {
            continue;
        }
        let key = layout::audit_key(month);
        s.remote.put(&key, s.keys.seal(&bytes, &key))?;
        conn.execute(
            "INSERT INTO local_state (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            [&k, &h],
        )?;
    }
    Ok(())
}
