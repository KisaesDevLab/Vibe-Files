//! Each person's undo list, kept in the cabinet too (owner 2026-11-14, Q-105): a session
//! host's catalog is new every morning, and with it yesterday's undo records were gone.
//!
//! `_Vibe/undo/<login>/<tx>.json` holds one undoable change: what the catalog's `undo_record`
//! row holds, and the line the undo list shows for it. [`save`] writes the ones a login's
//! list has that the cabinet has not, removes the ones undone, and keeps the newest
//! [`KEEP`] plus everything from the last [`DAYS`] days (owner 2026-11-13, Q-102).
//! [`restore`] gives a catalog the records it does not know, when a cabinet opens.
//!
//! A restored record is undone like any other: its files go back, and its catalog statements
//! run against documents known by the same ids on every PC (their folder's `docmeta.jsonl`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vf_core::db::Connection;

use crate::cabinet;

const DIR: &str = "undo";
/// Records kept per person whatever their age.
pub const KEEP: usize = vf_core::tx::UNDO_DEPTH;
/// Days a record is kept beyond the newest [`KEEP`].
pub const DAYS: i64 = 30;

#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    tx_id: String,
    #[serde(default)]
    user: Option<String>,
    device_id: String,
    ts: String,
    label: String,
    #[serde(default)]
    summary: String,
    record: serde_json::Value,
}

/// The folder of the login (or, with none, the PC) working as `device_id`.
fn dir(root: &Path, device_id: &str) -> PathBuf {
    let who = vf_core::windows_login().unwrap_or_else(|| device_id.to_string());
    let safe: String = who
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    root.join(cabinet::VIBE_DIR).join(DIR).join(safe)
}

fn on_disk(dir: &Path) -> Vec<(String, PathBuf)> {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let id = p.file_name()?.to_str()?.strip_suffix(".json")?.to_string();
                    Some((id, p))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Brings the cabinet's copy of this login's undo list in line with the catalog's: new
/// records written, undone ones removed, old ones pruned. Returns how many were written.
pub fn save(conn: &Connection, root: &Path, device_id: &str) -> Result<usize, String> {
    let stack = crate::fileops::undo_stack(conn, device_id)?;
    let dir = dir(root, device_id);
    let have = on_disk(&dir);
    let mut written = 0;
    for u in &stack {
        if have.iter().any(|(id, _)| id == &u.tx_id) {
            continue;
        }
        let row: Option<(Option<String>, String, String)> = conn
            .query_row(
                "SELECT user, device_id, record FROM undo_record WHERE tx_id = ?1",
                [&u.tx_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((user, device, record)) = row else {
            continue;
        };
        let saved = Saved {
            tx_id: u.tx_id.clone(),
            user,
            device_id: device,
            ts: u.ts.clone(),
            label: u.label.clone(),
            summary: u.summary.clone(),
            record: serde_json::from_str(&record).map_err(|e| e.to_string())?,
        };
        cabinet::write_replace(
            &dir.join(format!("{}.json", u.tx_id)),
            &serde_json::to_vec(&saved).expect("serializes"),
        )?;
        written += 1;
    }
    // Undone here: off the cabinet's list too.
    for (id, path) in &have {
        let undone = conn
            .query_row(
                "SELECT undone FROM undo_record WHERE tx_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .is_ok_and(|u| u != 0);
        if undone {
            let _ = std::fs::remove_file(path);
        }
    }
    if written > 0 && have.len() + written > KEEP {
        prune(&dir);
    }
    Ok(written)
}

/// Keeps the newest [`KEEP`] records and those of the last [`DAYS`] days.
fn prune(dir: &Path) {
    let mut all: Vec<(String, PathBuf)> = on_disk(dir)
        .into_iter()
        .filter_map(|(_, p)| {
            let s: Saved = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((s.ts, p))
        })
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0));
    let cutoff = (time::OffsetDateTime::now_utc() - time::Duration::days(DAYS))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    for (ts, p) in all.into_iter().skip(KEEP) {
        if ts < cutoff {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Gives this catalog the records of this login's undo list it does not know (a catalog made
/// this morning), oldest first so the list keeps its order. Returns how many came in.
pub fn restore(conn: &mut Connection, root: &Path, device_id: &str) -> Result<usize, String> {
    let mut saved: Vec<Saved> = on_disk(&dir(root, device_id))
        .into_iter()
        .filter_map(|(_, p)| serde_json::from_slice(&std::fs::read(p).ok()?).ok())
        .collect();
    if saved.is_empty() {
        return Ok(0);
    }
    saved.sort_by(|a, b| a.ts.cmp(&b.ts));
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for s in saved {
        // The line the list shows travels in the record: this catalog has no audit rows for
        // it to be read from.
        let mut record = s.record;
        if let Some(o) = record.as_object_mut() {
            o.insert("summary".into(), s.summary.into());
        }
        n += tx
            .execute(
                "INSERT OR IGNORE INTO undo_record (tx_id, user, device_id, ts, label, record, undone)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                (
                    &s.tx_id,
                    &s.user,
                    // Under the id this login works under now (a wiped host has a new one).
                    device_id,
                    &s.ts,
                    &s.label,
                    record.to_string(),
                ),
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::db::Storage;

    /// A change made on one PC is on the undo list of a catalog made after it, with the line
    /// it was shown with, and is undone there; once undone it is off the cabinet's list.
    #[test]
    fn the_undo_list_survives_a_new_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cab");
        let firm = uuid::Uuid::new_v4();
        cabinet::create(
            &root,
            &cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: firm.to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let folder = root.join("Acme LLC (1001)");
        std::fs::create_dir_all(&folder).unwrap();
        filer_intake::sentinel::write_sentinel(
            &root,
            &folder,
            &filer_core::clients::Client {
                id: "1001".into(),
                uuid: uuid::Uuid::new_v4(),
                name: "Acme LLC".into(),
                extra: Default::default(),
            },
            firm,
        )
        .unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        let mut conn = vf_core::db::open(&tmp.path().join("a.db"), Storage::Local).unwrap();
        crate::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
        crate::folders::new_folder(&mut conn, &root, "Acme LLC (1001)", "Bank", "dev").unwrap();
        assert!(folder.join("Bank").is_dir());
        let stack = crate::fileops::undo_stack(&conn, "dev").unwrap();
        assert_eq!(stack.len(), 1);
        assert_eq!(save(&conn, &root, "dev").unwrap(), 1);
        assert_eq!(save(&conn, &root, "dev").unwrap(), 0, "written once");

        // The next morning: a new catalog, a new id for the host.
        let mut next = vf_core::db::open(&tmp.path().join("b.db"), Storage::Local).unwrap();
        crate::rebuild::rebuild(&mut next, &root, "dev-2").unwrap();
        assert!(
            crate::fileops::undo_stack(&next, "dev-2")
                .unwrap()
                .is_empty()
        );
        assert_eq!(restore(&mut next, &root, "dev-2").unwrap(), 1);
        assert_eq!(restore(&mut next, &root, "dev-2").unwrap(), 0);
        let again = crate::fileops::undo_stack(&next, "dev-2").unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].tx_id, stack[0].tx_id);
        assert_eq!(again[0].summary, stack[0].summary);
        assert!(!again[0].summary.is_empty());
        crate::fileops::undo(&mut next, &root, &again[0].tx_id, "dev-2").unwrap();
        assert!(!folder.join("Bank").exists());
        assert!(
            crate::fileops::undo_stack(&next, "dev-2")
                .unwrap()
                .is_empty()
        );
        save(&next, &root, "dev-2").unwrap();
        assert!(on_disk(&dir(&root, "dev-2")).is_empty());
    }
}
