//! Legal holds (PHASES P07 step 4): a client, or a set of documents, is frozen: nothing under
//! a hold is disposed of or emptied from the trash until the hold is released.
//!
//! The holds live in the cabinet, `<cabinet>/_Vibe/legal_holds.json`, because each workstation
//! keeps its own catalog (Q-26): a hold placed on one PC must stop a disposition run on
//! another. Targets are portable: a client by its sentinel uuid, a document by its cabinet path
//! and content hash (catalog ids differ between workstations). [`sync`] mirrors the file into
//! the catalog's `legal_hold` rows (run when a cabinet opens, before anything is emptied);
//! anything destructive checks the file itself with [`covers`] first.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};

use crate::clients::master_dir;

pub const FILE: &str = "legal_holds.json";
/// Transaction label (placing / releasing is undone by releasing / placing again, not by the
/// undo stack, so the cabinet file and the catalog never disagree).
pub const LABEL: &str = "legal_hold";

/// A document a hold covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRef {
    pub rel_path: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub id: String,
    /// Client uuid (sentinel `client_id`): every document of the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Or these documents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub docs: Vec<DocRef>,
    pub reason: String,
    pub since: String,
    #[serde(default)]
    pub placed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_by: Option<String>,
}

impl Hold {
    pub fn active(&self) -> bool {
        self.released.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileV1 {
    version: u32,
    holds: Vec<Hold>,
}

pub fn path(root: &Path) -> PathBuf {
    master_dir(root).join(FILE)
}

/// Every hold, released ones too (they are the record).
pub fn load(root: &Path) -> Result<Vec<Hold>, String> {
    match std::fs::read_to_string(path(root)) {
        Ok(t) => {
            let f: FileV1 =
                serde_json::from_str(&t).map_err(|e| format!("{FILE} could not be read: {e}"))?;
            if f.version != 1 {
                return Err(format!(
                    "{FILE} is version {}; this build reads 1",
                    f.version
                ));
            }
            Ok(f.holds)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{FILE} could not be read: {e}")),
    }
}

/// Merges two `legal_holds.json` texts (two workstations changed holds since their last sync):
/// every hold from either side; a hold released on either side stays released. `None` when
/// either side cannot be read.
pub fn merge_text(a: &str, b: &str) -> Option<String> {
    let pa: FileV1 = serde_json::from_str(a).ok()?;
    let pb: FileV1 = serde_json::from_str(b).ok()?;
    if pa.version != 1 || pb.version != 1 {
        return None;
    }
    let mut by: std::collections::BTreeMap<String, Hold> =
        pa.holds.into_iter().map(|h| (h.id.clone(), h)).collect();
    for h in pb.holds {
        match by.get_mut(&h.id) {
            Some(x) if x.released.is_none() && h.released.is_some() => *x = h,
            Some(_) => {}
            None => {
                by.insert(h.id.clone(), h);
            }
        }
    }
    let f = FileV1 {
        version: 1,
        holds: by.into_values().collect(),
    };
    Some(serde_json::to_string_pretty(&f).expect("serializes"))
}

fn save(root: &Path, holds: &[Hold]) -> Result<(), String> {
    let f = FileV1 {
        version: 1,
        holds: holds.to_vec(),
    };
    crate::cabinet::write_replace(
        &path(root),
        &serde_json::to_vec_pretty(&f).expect("serializes"),
    )
    .map_err(|e| format!("saving {FILE}: {e}"))
}

/// Whether an active hold in the cabinet file covers a document of `client_id` at `rel_path`
/// with content `hash`. A file that cannot be read counts as covering everything: when in
/// doubt, nothing is destroyed.
pub fn covers(root: &Path, client_id: Option<&str>, rel_path: &str, hash: &str) -> bool {
    let Ok(holds) = load(root) else {
        return true;
    };
    holds.iter().filter(|h| h.active()).any(|h| {
        (h.client_id.is_some() && h.client_id.as_deref() == client_id)
            || h.docs
                .iter()
                .any(|d| d.hash == hash || d.rel_path == rel_path)
    })
}

/// One catalog row a hold needs.
struct Row {
    id: String,
    client: Option<String>,
    doc: Option<String>,
    reason: String,
    since: String,
    released: Option<String>,
}

/// Catalog rows for the file's holds: one per client hold, one per held document found in
/// this catalog (by hash or path). Released holds carry `released`.
fn rows(conn: &Connection, holds: &[Hold]) -> Vec<Row> {
    let mut out = vec![];
    for h in holds {
        let row = |id: String, client, doc| Row {
            id,
            client,
            doc,
            reason: h.reason.clone(),
            since: h.since.clone(),
            released: h.released.clone(),
        };
        if let Some(c) = &h.client_id {
            // Only when this catalog knows the client (the foreign key).
            let known = conn
                .query_row("SELECT 1 FROM client WHERE id = ?1", [c], |_| Ok(()))
                .is_ok();
            if known {
                out.push(row(h.id.clone(), Some(c.clone()), None));
            }
        }
        for (i, d) in h.docs.iter().enumerate() {
            let doc: Option<String> = conn
                .query_row(
                    "SELECT id FROM document WHERE hash = ?1 OR rel_path = ?2
                     ORDER BY (hash = ?1) DESC, deleted_at IS NULL DESC LIMIT 1",
                    [&d.hash, &d.rel_path],
                    |r| r.get(0),
                )
                .ok();
            if let Some(doc) = doc {
                out.push(row(format!("{}/{i}", h.id), None, Some(doc)));
            }
        }
    }
    out
}

fn stmt(r: &Row, device_id: &str, now: &str) -> Stmt {
    Stmt::new(
        "INSERT INTO legal_hold (id, client_id, doc_id, reason, since, released, device_id,
           updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET reason = excluded.reason,
           released = excluded.released, device_id = excluded.device_id,
           updated_at = excluded.updated_at, rev = legal_hold.rev + 1",
        vec![
            r.id.clone().into(),
            r.client.clone().into(),
            r.doc.clone().into(),
            r.reason.clone().into(),
            r.since.clone().into(),
            r.released.clone().into(),
            device_id.into(),
            now.into(),
        ],
    )
}

/// Whether the catalog already has `r` as it is.
fn current(conn: &Connection, r: &Row) -> bool {
    conn.query_row(
        "SELECT reason, released FROM legal_hold WHERE id = ?1",
        [&r.id],
        |q| Ok((q.get::<_, String>(0)?, q.get::<_, Option<String>>(1)?)),
    )
    .is_ok_and(|(reason, released)| reason == r.reason && released == r.released)
}

/// Mirrors the cabinet's holds into this catalog (one audited Transaction when anything
/// changed). Returns how many holds are active.
pub fn sync(conn: &mut Connection, root: &Path, device_id: &str) -> Result<usize, String> {
    let holds = load(root)?;
    let active = holds.iter().filter(|h| h.active()).count();
    let now = vf_core::now_rfc3339();
    let changed: Vec<Row> = rows(conn, &holds)
        .into_iter()
        .filter(|r| !current(conn, r))
        .collect();
    if changed.is_empty() {
        return Ok(active);
    }
    let mut tx = Transaction::new(LABEL).audit(AuditEntry::new(
        "legal_hold_sync",
        None,
        json!({ "holds": holds.len(), "active": active, "rows": changed.len() }),
    ));
    for r in &changed {
        tx = tx.catalog(stmt(r, device_id, &now));
    }
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    Ok(active)
}

/// What a new hold covers.
pub enum Target {
    Client(String),
    /// Catalog document ids (stored by path and hash).
    Docs(Vec<String>),
}

/// Places a hold: the cabinet file first, then the catalog, one audit row.
pub fn place(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    target: Target,
    reason: &str,
) -> Result<String, String> {
    if reason.trim().is_empty() {
        return Err("Say why (a matter, an examination, a request).".into());
    }
    let id = vf_core::new_id();
    let mut hold = Hold {
        id: id.clone(),
        client_id: None,
        docs: vec![],
        reason: reason.trim().into(),
        since: vf_core::now_rfc3339(),
        placed_by: vf_core::windows_login(),
        released: None,
        released_by: None,
    };
    let detail = match target {
        Target::Client(c) => {
            hold.client_id = Some(c.clone());
            json!({ "hold": id, "client": c, "reason": hold.reason })
        }
        Target::Docs(ids) => {
            for d in &ids {
                let (rel_path, hash): (String, String) = conn
                    .query_row(
                        "SELECT rel_path, hash FROM document WHERE id = ?1",
                        [d],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map_err(|_| "a document is not in the catalog".to_string())?;
                hold.docs.push(DocRef { rel_path, hash });
            }
            if hold.docs.is_empty() {
                return Err("nothing selected".into());
            }
            json!({ "hold": id, "documents": hold.docs.len(), "reason": hold.reason })
        }
    };
    let mut holds = load(root)?;
    holds.push(hold);
    save(root, &holds)?;
    write(conn, device_id, &holds, "legal_hold_place", detail)?;
    Ok(id)
}

/// Releases hold `id`.
pub fn release(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    id: &str,
) -> Result<(), String> {
    let mut holds = load(root)?;
    let h = holds
        .iter_mut()
        .find(|h| h.id == id && h.active())
        .ok_or("That hold is not active any more.")?;
    h.released = Some(vf_core::now_rfc3339());
    h.released_by = vf_core::windows_login();
    let detail = json!({ "hold": id, "reason": h.reason });
    save(root, &holds)?;
    write(conn, device_id, &holds, "legal_hold_release", detail)
}

fn write(
    conn: &mut Connection,
    device_id: &str,
    holds: &[Hold],
    action: &str,
    detail: serde_json::Value,
) -> Result<(), String> {
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL).audit(AuditEntry::new(action, None, detail));
    for r in rows(conn, holds) {
        tx = tx.catalog(stmt(&r, device_id, &now));
    }
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Active holds that cover client `client_id` itself, or any of `doc_ids` (catalog ids), for
/// Browse's banner.
pub fn active_for(
    root: &Path,
    conn: &Connection,
    client_id: Option<&str>,
    doc_ids: &[String],
) -> Vec<Hold> {
    let Ok(holds) = load(root) else {
        return vec![];
    };
    let docs: Vec<(String, String, Option<String>)> = doc_ids
        .iter()
        .filter_map(|d| {
            conn.query_row(
                "SELECT rel_path, hash, client_id FROM document WHERE id = ?1",
                [d],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
        })
        .collect();
    holds
        .into_iter()
        .filter(|h| h.active())
        .filter(|h| {
            let by_client = h.client_id.is_some()
                && (h.client_id.as_deref() == client_id
                    || docs
                        .iter()
                        .any(|(_, _, c)| c.as_deref() == h.client_id.as_deref()));
            let by_doc = h.docs.iter().any(|r| {
                docs.iter()
                    .any(|(p, hash, _)| *p == r.rel_path || *hash == r.hash)
            });
            by_client || by_doc
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    /// A cabinet with one client and two documents, and two workstations' catalogs.
    fn fx() -> (tempfile::TempDir, PathBuf, Connection, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        let open = |name: &str| {
            let c = vf_core::db::open(&t.path().join(name), Storage::Local).unwrap();
            c.execute_batch(
                "INSERT INTO client (id, ext_id, name, device_id, updated_at)
                   VALUES ('c1', 'ANDE1042', 'Anderson Farms LLC', 'd', 't'),
                          ('c2', 'BELL2002', 'Bell Ranch', 'd', 't');
                 INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at)
                   VALUES ('a', 'c1', 'A/w2.pdf', 'ha', 1, 't', 'd', 't'),
                          ('b', 'c2', 'B/k1.pdf', 'hb', 1, 't', 'd', 't');",
            )
            .unwrap();
            c
        };
        let (pc1, pc2) = (open("pc1.db"), open("pc2.db"));
        (t, root, pc1, pc2)
    }

    #[test]
    fn client_and_document_holds_cover_across_workstations() {
        let (_t, root, mut pc1, mut pc2) = fx();
        let client = place(
            &mut pc1,
            &root,
            "pc1",
            Target::Client("c1".into()),
            "IRS exam 2023",
        )
        .unwrap();
        place(
            &mut pc1,
            &root,
            "pc1",
            Target::Docs(vec!["b".into()]),
            "lawsuit",
        )
        .unwrap();
        assert!(covers(&root, Some("c1"), "A/w2.pdf", "ha"));
        assert!(covers(&root, Some("c2"), "B/k1.pdf", "hb"));
        assert!(
            covers(&root, Some("c2"), "B/moved.pdf", "hb"),
            "by content after a move"
        );
        assert!(!covers(&root, Some("c2"), "B/other.pdf", "hz"));
        assert!(place(&mut pc1, &root, "pc1", Target::Client("c2".into()), " ").is_err());

        // The other PC sees them once it syncs (at open), and its trash honors the file before.
        assert_eq!(sync(&mut pc2, &root, "pc2").unwrap(), 2);
        let rows: i64 = pc2
            .query_row(
                "SELECT count(*) FROM legal_hold WHERE released IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 2);
        let audits = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT count(*) FROM audit WHERE action = 'legal_hold_sync'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(audits(&pc2), 1);
        sync(&mut pc2, &root, "pc2").unwrap();
        assert_eq!(audits(&pc2), 1, "no change, no audit row");

        let banner = active_for(&root, &pc1, None, &["a".to_string()]);
        assert_eq!(banner.len(), 1);
        assert_eq!(banner[0].reason, "IRS exam 2023");

        release(&mut pc1, &root, "pc1", &client).unwrap();
        assert!(!covers(&root, Some("c1"), "A/w2.pdf", "ha"));
        assert!(release(&mut pc1, &root, "pc1", &client).is_err());
        sync(&mut pc2, &root, "pc2").unwrap();
        let active: i64 = pc2
            .query_row(
                "SELECT count(*) FROM legal_hold WHERE released IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 1);
        assert_eq!(
            load(&root).unwrap().len(),
            2,
            "released holds stay on record"
        );
    }

    #[test]
    fn an_unreadable_hold_file_holds_everything() {
        let (_t, root, _pc1, _pc2) = fx();
        assert!(!covers(&root, Some("c1"), "A/w2.pdf", "ha"));
        std::fs::write(path(&root), "{ not json").unwrap();
        assert!(covers(&root, Some("c1"), "A/w2.pdf", "ha"));
    }
}
