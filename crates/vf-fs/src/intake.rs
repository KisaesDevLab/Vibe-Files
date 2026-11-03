//! The catalog side of Filer Desktop's Intake commits (P02 step 1).
//!
//! The mounted Filer app files documents itself (`filer_intake::commit::run_commit`: Filer's
//! moves, collisions, copies, sentinels, `ops.jsonl`, `last_batch.json`). Vibe Files then records
//! each finished batch in the catalog as **one** Transaction, as CLAUDE.md §Document moves
//! describes: a `document` row (and a `sync_pending` row) per filed file, audit rows from the
//! batch's `ops.jsonl` records, and an undo record whose catalog inverse is run when Filer's own
//! Undo takes the batch back. Files first, catalog second: a crash in between is caught up when
//! the cabinet next opens ([`reconcile`]), and `rebuild` reconciles anything else.

use std::collections::HashMap;
use std::path::Path;

use filer_intake::commit::{LastBatch, Method, load_last_batch};
use filer_intake::oplog::{self, OpResult};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, FileOp, NoFileOps, Stmt, Transaction};

use crate::hints::Hint;

use crate::rebuild::{mime, rel, year_from_path};

pub const LABEL: &str = "intake_batch";

/// What [`reconcile`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// A batch was recorded: (Filer batch id, documents added, documents updated).
    pub recorded: Option<(String, usize, usize)>,
    /// A batch Filer undid had its catalog side undone.
    pub undone: Option<String>,
}

impl Reconciled {
    pub fn changed(&self) -> bool {
        self.recorded.is_some() || self.undone.is_some()
    }
}

/// The undo record of a recorded Filer batch: (tx id, already undone).
fn recorded(conn: &Connection, batch: &str) -> Option<(String, bool)> {
    conn.query_row(
        "SELECT tx_id, undone FROM undo_record
         WHERE label = ?1 AND json_extract(record, '$.extra.filer_batch') = ?2",
        [LABEL, batch],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// Brings the catalog in line with Filer's last batch: records a finished batch not recorded
/// yet, or undoes the catalog side of a batch Filer undid. Cheap when there is nothing to do.
pub fn reconcile(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
) -> Result<Reconciled, String> {
    let dir = crate::clients::master_dir(root);
    let Some(lb) = load_last_batch(&dir) else {
        return Ok(Reconciled::default());
    };
    if lb.in_progress {
        return Ok(Reconciled::default()); // still filing (or crashed mid-batch: Filer recovers)
    }
    let batch = lb.batch.to_string();
    match (recorded(conn, &batch), lb.undone) {
        (None, false) => {
            let (added, updated) = record(conn, root, &lb, device_id)?;
            Ok(Reconciled {
                recorded: Some((batch, added, updated)),
                ..Default::default()
            })
        }
        (Some((tx, false)), true) => {
            // Filer already moved the files back: run only the catalog inverse.
            vf_core::tx::undo(conn, &mut NoFileOps, &Actor::device(device_id), &tx)
                .map_err(|e| e.to_string())?;
            Ok(Reconciled {
                undone: Some(batch),
                ..Default::default()
            })
        }
        // Recorded and in step, or undone before it was ever recorded: nothing to do.
        _ => Ok(Reconciled::default()),
    }
}

/// Records one finished, not-undone batch. Returns (documents added, documents updated).
fn record(
    conn: &mut Connection,
    root: &Path,
    lb: &LastBatch,
    device_id: &str,
) -> Result<(usize, usize), String> {
    // A batch can create client folders and sentinels: catalog clients first.
    crate::rebuild::sync_clients(conn, root, device_id, "intake_clients")
        .map_err(|e| e.to_string())?;
    // CSV ID → (client id, client folder).
    let clients: HashMap<String, (String, Option<String>)> = conn
        .prepare("SELECT ext_id, id, folder_rel FROM client WHERE deleted_at IS NULL")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
                .collect()
        })
        .map_err(|e| e.to_string())?;
    let doc_types: HashMap<String, String> = conn
        .prepare("SELECT id, name FROM doc_type WHERE deleted_at IS NULL")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(|e| e.to_string())?;
    let hints = crate::hints::load(root);

    // Every file the batch put in the cabinet: each item's destination and its copies.
    struct Filed<'a> {
        rel: String,
        client: Option<&'a (String, Option<String>)>,
        bytes: u64,
        sha256: Option<String>,
    }
    let mut filed: Vec<Filed> = Vec::new();
    for it in &lb.items {
        filed.push(Filed {
            rel: rel(root, &it.dst),
            client: clients.get(&it.client_id),
            bytes: it.bytes,
            sha256: None,
        });
        for c in &it.copies {
            filed.push(Filed {
                rel: rel(root, &c.dst),
                client: clients.get(&c.client_id),
                bytes: c.bytes,
                sha256: Some(c.sha256.clone()),
            });
        }
    }

    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL).extra(json!({ "filer_batch": lb.batch.to_string() }));
    let (mut added, mut updated) = (0, 0);
    for f in &filed {
        let path = root.join(&f.rel);
        if !path.is_file() {
            continue; // a later batch or a person moved it; rebuild reconciles
        }
        // A rename on one volume carries no hash; hash what is in the cabinet now.
        let hash = match &f.sha256 {
            Some(h) => h.clone(),
            None => filer_intake::commit::sha256_file(&path)
                .map_err(|e| format!("cannot hash {}: {e}", path.display()))?,
        };
        let existing: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT id, hash, size FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                [&f.rel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        match existing {
            // Overwrite: the cataloged document now has new content; the old content is a
            // version (Filer kept the bytes in `_Vibe/overwritten/<batch>/`, Q-17).
            Some((id, old_hash, old_size)) => {
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                         VALUES (?1, ?2, ?3, ?4, 'intake', ?5, 'overwritten by intake')",
                        vec![
                            id.clone().into(),
                            old_hash.clone().into(),
                            old_size.into(),
                            now.clone().into(),
                            device_id.into(),
                        ],
                    ))
                    .catalog(Stmt::new(
                        "UPDATE document SET hash = ?2, size = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1
                         WHERE id = ?1",
                        vec![
                            id.clone().into(),
                            hash.clone().into(),
                            (f.bytes as i64).into(),
                            device_id.into(),
                            now.clone().into(),
                        ],
                    ))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET hash = ?2, size = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1
                         WHERE id = ?1",
                        vec![
                            id.clone().into(),
                            old_hash.into(),
                            old_size.into(),
                            device_id.into(),
                            now.clone().into(),
                        ],
                    ));
                tx = tx.catalog(sync_put(&id, &now));
                updated += 1;
            }
            None => {
                let id = vf_core::new_id();
                // What a Vibe intake processor knew about the file (barcode cover sheet, print
                // capture parser): doc type, tax year, source. Kept in docmeta.jsonl too, so
                // rebuild restores it (Q-22).
                let hint = hints.get(&hash);
                let doc_type = hint
                    .and_then(|h| h.doc_type.clone())
                    .filter(|t| doc_types.contains_key(t));
                let tax_year = match hint {
                    Some(h) if h.permanent => None,
                    Some(Hint {
                        tax_year: Some(y), ..
                    }) => Some(i64::from(*y)),
                    _ => year_from_path(&f.rel),
                };
                let source = hint
                    .and_then(|h| h.source.clone())
                    .unwrap_or_else(|| "client".into());
                if hint.is_some_and(Hint::is_useful) {
                    let meta = crate::docmeta::Meta {
                        id: id.clone(),
                        rel_path: f.rel.clone(),
                        hash: hash.clone(),
                        folder_rel: f.client.and_then(|c| c.1.clone()),
                        doc_type_name: doc_type.as_ref().and_then(|t| doc_types.get(t).cloned()),
                        doc_type: doc_type.clone(),
                        tax_year: tax_year.map(|y| y as i32),
                        status: "received".into(),
                        source: Some(source.clone()),
                        tags: vec![],
                    };
                    let (path, line) = crate::docmeta::record_line(root, &meta, &now, device_id);
                    tx = tx.file(FileOp::AppendLines {
                        path,
                        lines: vec![line],
                        undo_lines: vec![],
                    });
                }
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, mime, status, source,
                                               filed_by, filed_at, device_id, updated_at, doc_type_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'received', ?10, 'intake', ?8, ?9, ?8, ?11)",
                        vec![
                            id.clone().into(),
                            f.client.map(|c| c.0.clone()).into(),
                            tax_year.into(),
                            f.rel.clone().into(),
                            hash.into(),
                            (f.bytes as i64).into(),
                            mime(&f.rel).into(),
                            now.clone().into(),
                            device_id.into(),
                            source.into(),
                            doc_type.into(),
                        ],
                    ))
                    .catalog(sync_put(&id, &now))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET deleted_at = ?2, tombstone_reason = 'intake undone',
                           device_id = ?3, updated_at = ?2, rev = rev + 1
                         WHERE id = ?1",
                        vec![id.clone().into(), now.clone().into(), device_id.into()],
                    ));
                added += 1;
            }
        }
    }

    // Filer's op log for this batch becomes audit rows (one per record).
    let ops = oplog::read(&oplog::path(&crate::clients::master_dir(root)))
        .map(|r| r.records)
        .unwrap_or_default();
    let mut audited = 0;
    for op in ops.iter().filter(|o| o.batch == lb.batch) {
        let action = serde_json::to_value(op.op)
            .ok()
            .and_then(|v| v.as_str().map(|s| format!("intake_{s}")))
            .unwrap_or_else(|| "intake_op".into());
        let path_rel = |p: &Option<std::path::PathBuf>| p.as_ref().map(|p| rel(root, p));
        tx = tx.audit(AuditEntry::new(
            &action,
            None,
            json!({
                "filer_batch": lb.batch.to_string(),
                "client": op.client_id,
                "src": op.src.as_ref().map(|p| p.display().to_string()),
                "dst": path_rel(&op.dst),
                "bytes": op.bytes,
                "sha256": op.sha256,
                "ok": op.result == OpResult::Ok,
                "error": op.error,
            }),
        ));
        audited += 1;
    }
    let moved = lb
        .items
        .iter()
        .filter(|i| matches!(i.method, Method::Move | Method::CopyVerifyDelete))
        .count();
    tx = tx.audit(AuditEntry::new(
        LABEL,
        None,
        json!({
            "filer_batch": lb.batch.to_string(),
            "files": moved,
            "documents_added": added,
            "documents_updated": updated,
            "op_records": audited,
        }),
    ));
    // The only file operations are docmeta lines for hinted documents (Filer moved the files).
    crate::fileops::commit(conn, root, tx, device_id)?;

    // Q-17: keep overwritten content as versions in `_Vibe/versions/` (content-addressed, with a
    // sidecar so rebuild restores the version rows). Filer's own copy stays for its undo.
    for b in &lb.backups {
        let _ = keep_overwritten(root, &b.original, &b.kept, device_id, &now);
    }
    Ok((added, updated))
}

fn sync_put(id: &str, now: &str) -> Stmt {
    Stmt::new(
        "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2)
         ON CONFLICT(doc_id, op) DO NOTHING",
        vec![id.into(), now.into()],
    )
}

/// Copies an overwritten file's kept bytes into `_Vibe/versions/<hash>` (once) and adds a
/// sidecar entry for the document path it was overwritten at.
fn keep_overwritten(
    root: &Path,
    original: &Path,
    kept: &Path,
    device_id: &str,
    now: &str,
) -> Result<(), String> {
    let hash = filer_intake::commit::sha256_file(kept).map_err(|e| e.to_string())?;
    let dst = crate::cabinet::version_path(root, &hash);
    if !dst.exists() {
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        filer_intake::commit::copy_verified(kept, &dst)?;
    }
    let sidecar = dst.with_extension("json");
    let mut entries: Vec<crate::cabinet::VersionSidecar> = std::fs::read_to_string(&sidecar)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    entries.push(crate::cabinet::VersionSidecar {
        rel_path: rel(root, original),
        created: now.into(),
        created_by: Some("intake".into()),
        device_id: device_id.into(),
        reason: Some("overwritten by intake".into()),
    });
    crate::cabinet::write_replace(
        &sidecar,
        &serde_json::to_vec_pretty(&entries).expect("serializes"),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    use filer_core::clients::Client;
    use filer_intake::commit::{CommitInput, CommitItem, CommitReport, run_commit};
    use filer_intake::intake::Source;
    use filer_intake::plan::FolderAction;
    use vf_core::db::{CatalogKey, Storage};

    use super::*;
    use crate::cabinet::{self, CabinetFile};

    struct Fx {
        _t: tempfile::TempDir,
        root: PathBuf,
        inbox: PathBuf,
        conn: Connection,
        firm: uuid::Uuid,
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cabinet");
        let inbox = t.path().join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let firm = uuid::Uuid::new_v4();
        cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: firm.to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), &CatalogKey([7; 32]), Storage::Local)
            .unwrap();
        Fx {
            _t: t,
            root,
            inbox,
            conn,
            firm,
        }
    }

    fn client(id: &str, name: &str) -> Client {
        Client {
            id: id.into(),
            uuid: uuid::Uuid::new_v4(),
            name: name.into(),
            extra: BTreeMap::new(),
        }
    }

    /// Files `names` from the inbox into the folder for `c` with Filer's own commit.
    fn file(f: &Fx, c: &Client, names: &[&str], overwrite: bool) -> CommitReport {
        let folder = f.root.join(format!("{} ({})", c.name, c.id));
        let action = if folder.exists() {
            FolderAction::Exists(folder.clone())
        } else {
            FolderAction::WillCreate(folder.clone())
        };
        let items = names
            .iter()
            .map(|name| {
                let src = f.inbox.join(name);
                if !src.exists() {
                    std::fs::write(&src, format!("content of {name}")).unwrap();
                }
                CommitItem {
                    src,
                    dest: folder.join("Income Tax/2025").join(name),
                    folder: action.clone(),
                    client: c.clone(),
                    overwrite,
                    source: Source::Inbox(f.inbox.clone()),
                    merged_from: vec![],
                    split_of: None,
                    copies: vec![],
                    kept_both: false,
                    also_dirs: vec![],
                }
            })
            .collect();
        let input = CommitInput {
            batch: uuid::Uuid::new_v4(),
            items,
            skipped: vec![],
            root: f.root.clone(),
            first_inbox: Some(f.inbox.clone()),
            data_dir: crate::clients::master_dir(&f.root),
            firm_id: f.firm,
            template: filer_core::template::default_tree(),
            splits: vec![],
            force_copy: false,
        };
        let rep = run_commit(&input, &AtomicUsize::new(0));
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        rep
    }

    fn n(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn w2(f: &Fx) -> PathBuf {
        f.root.join("Acme LLC (1001)/Income Tax/2025/w2.pdf")
    }

    /// A Filer batch becomes one Transaction: document + sync rows, audit rows from ops.jsonl,
    /// an undo record keyed by the Filer batch; reconcile is idempotent.
    #[test]
    fn filer_batch_is_recorded_once() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        let rep = file(&f, &acme, &["w2.pdf", "1099-int.pdf"], false);
        let r = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let batch = rep.last_batch.batch.to_string();
        assert_eq!(r.recorded, Some((batch, 2, 0)));

        let acme_id: String = f
            .conn
            .query_row("SELECT id FROM client WHERE ext_id = '1001'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(acme_id, acme.uuid.to_string(), "bound by Filer's sentinel");
        let sql = format!(
            "SELECT count(*) FROM document WHERE client_id = '{acme_id}' AND tax_year = 2025
             AND status = 'received' AND filed_by = 'intake' AND deleted_at IS NULL"
        );
        assert_eq!(n(&f.conn, &sql), 2);
        let hash: String = f
            .conn
            .query_row(
                "SELECT hash FROM document WHERE rel_path LIKE '%/w2.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hash, filer_intake::commit::sha256_file(&w2(&f)).unwrap());
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM sync_pending WHERE op = 'put'"
            ),
            2
        );
        assert!(
            n(
                &f.conn,
                "SELECT count(*) FROM audit WHERE action LIKE 'intake_%'"
            ) >= 3
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM undo_record WHERE label = 'intake_batch'"
            ),
            1
        );

        // Again: nothing more.
        assert!(!reconcile(&mut f.conn, &f.root, "dev").unwrap().changed());
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NULL"
            ),
            2
        );
    }

    /// Filer's own Undo moves the files back; reconcile then undoes the catalog side only.
    #[test]
    fn filer_undo_tombstones_the_batch() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        let rep = file(&f, &acme, &["w2.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let undo = filer_intake::undo::run_undo(
            &crate::clients::master_dir(&f.root),
            rep.last_batch.clone(),
            false,
        );
        assert!(undo.last_batch.undone);
        assert!(f.inbox.join("w2.pdf").exists(), "Filer put the file back");

        let r = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert_eq!(r.undone, Some(rep.last_batch.batch.to_string()));
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NULL"
            ),
            0
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE tombstone_reason = 'intake undone'"
            ),
            1
        );
        assert!(!reconcile(&mut f.conn, &f.root, "dev").unwrap().changed());
    }

    /// A hint from a Vibe intake processor (barcode cover sheet, print capture) sets the doc
    /// type, year and source of the document it names by hash, and is kept in docmeta.jsonl.
    #[test]
    fn hints_set_doc_type_year_and_source() {
        let mut f = fx();
        crate::doctypes::seed_if_missing(&f.root).unwrap();
        crate::doctypes::sync(&mut f.conn, &f.root, "dev", "seed").unwrap();
        let w2_type = crate::doctypes::load(&f.root).unwrap().unwrap()[0].clone();
        assert_eq!(w2_type.name, "W-2");
        std::fs::write(f.inbox.join("w2.pdf"), "content of w2.pdf").unwrap();
        let hash = filer_intake::commit::sha256_file(&f.inbox.join("w2.pdf")).unwrap();
        let mut h = crate::hints::Hint::new(&hash, "w2.pdf", "print_capture:test", "dev");
        h.doc_type = Some(w2_type.id.clone());
        h.tax_year = Some(2024);
        h.source = Some("firm".into());
        crate::hints::append(&f.root, &h).unwrap();

        let acme = client("1001", "Acme LLC");
        file(&f, &acme, &["w2.pdf", "other.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let row = |name: &str| -> (Option<String>, Option<i64>, String) {
            f.conn
                .query_row(
                    "SELECT doc_type_id, tax_year, source FROM document WHERE rel_path LIKE ?1",
                    [format!("%/{name}")],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        };
        assert_eq!(
            row("w2.pdf"),
            (Some(w2_type.id.clone()), Some(2024), "firm".into())
        );
        assert_eq!(row("other.pdf"), (None, Some(2025), "client".into()));
        let meta =
            std::fs::read_to_string(f.root.join("Acme LLC (1001)/_Vibe/docmeta.jsonl")).unwrap();
        assert_eq!(meta.lines().count(), 1);
        assert!(meta.contains(&w2_type.id) && meta.contains("\"firm\""));
    }

    /// Overwrite: the cataloged document gets the new content; the old content becomes a
    /// version in `_Vibe/versions/` with a sidecar (Q-17).
    #[test]
    fn overwrite_keeps_the_old_content_as_a_version() {
        let mut f = fx();
        let acme = client("1001", "Acme LLC");
        file(&f, &acme, &["w2.pdf"], false);
        reconcile(&mut f.conn, &f.root, "dev").unwrap();
        let old = filer_intake::commit::sha256_file(&w2(&f)).unwrap();
        std::fs::write(f.inbox.join("w2.pdf"), "corrected W-2").unwrap();
        file(&f, &acme, &["w2.pdf"], true);
        let r = reconcile(&mut f.conn, &f.root, "dev").unwrap();
        assert_eq!(r.recorded.map(|(_, a, u)| (a, u)), Some((0, 1)));
        assert_eq!(n(&f.conn, "SELECT count(*) FROM version"), 1);
        assert!(cabinet::version_path(&f.root, &old).is_file());
        assert!(
            cabinet::version_path(&f.root, &old)
                .with_extension("json")
                .is_file()
        );
        assert_eq!(
            n(
                &f.conn,
                "SELECT count(*) FROM document WHERE deleted_at IS NULL"
            ),
            1
        );
    }
}
