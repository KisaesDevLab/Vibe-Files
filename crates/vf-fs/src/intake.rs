//! Intake ↔ catalog (PHASES P02 step 1). The Intake tab is the vendored Filer Desktop engine;
//! this module is the Vibe Files side of its hooks (`filer_intake::app::Hooks`, VENDORED.md
//! change 5):
//!
//! - **Commit / unpack**: Filer's own `run_commit` / `run_unpack` files the batch (every move,
//!   copy, collision and the batch record are Filer's). Then one catalog Transaction records it:
//!   a `document` row per filed file and copy, the version of any file a Replace overwrote
//!   (QUESTIONS Q-17), client folders the batch created or bound, audit rows (Filer's ops become
//!   audit rows) and `sync_pending`. If that Transaction fails, Filer's `run_undo` puts the files
//!   back, so the batch lands in both places or in neither.
//! - **Undo**: Filer's `run_undo`, then the catalog follows the disk: rows of files that went
//!   back are tombstoned, overwritten files get their restored hash, removed client folders are
//!   unbound.
//! - [`prepare_filer_dir`] writes Filer's data folder (`<data>/filer/`) from the cabinet: root,
//!   firm id, folder template, client list (with the catalog's ids, so sentinels match) and
//!   relationships.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use filer_core::clients::{self as fclients, Client};
use filer_core::config::Settings;
use filer_core::template::{self, TemplateNode};
use filer_intake::app::Hooks;
use filer_intake::commit::{self, LastBatch, sha256_file};
use filer_intake::sentinel;
use filer_intake::undo;
use serde_json::json;
use uuid::Uuid;
use vf_core::audit::AuditEntry;
use vf_core::db::{self, CatalogKey, Connection, Storage};
use vf_core::tx::{Actor, Committed, FileOp, NoFileOps, Stmt, Transaction};

use crate::cabinet::{self, CabinetFile, VersionSidecar};
use crate::fileops::FilerFileOps;
use crate::rebuild::{mime, year_from_path};

/// What Vibe Files already knows about a queued file before Filer files it (from a barcode cover
/// sheet, a print-capture parser profile, …). Keyed by the file's queue path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hint {
    /// A doc type name (`doc_type.name`).
    pub doc_type: Option<String>,
    /// `client` / `firm` / `government` / `third_party`.
    pub source: Option<String>,
    pub tax_year: Option<i64>,
}

pub type Hints = Arc<Mutex<HashMap<PathBuf, Hint>>>;

/// Everything a hook needs to reach the catalog from Filer's worker thread.
#[derive(Clone)]
pub struct CatalogTarget {
    pub catalog: PathBuf,
    pub key: CatalogKey,
    pub root: PathBuf,
    pub actor: Actor,
    pub hints: Hints,
}

impl CatalogTarget {
    fn open(&self) -> vf_core::Result<Connection> {
        db::open(&self.catalog, &self.key, Storage::Local)
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn ext_map(conn: &Connection) -> vf_core::Result<HashMap<String, String>> {
    let mut st = conn.prepare("SELECT ext_id, id FROM client WHERE deleted_at IS NULL")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn doc_type_id(conn: &Connection, name: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM doc_type WHERE name = ?1 COLLATE NOCASE AND deleted_at IS NULL",
        [name],
        |r| r.get(0),
    )
    .ok()
}

/// One cabinet file the batch wrote.
struct Written<'a> {
    action: &'static str,
    src: Option<&'a Path>,
    dst: &'a Path,
    client_ext: &'a str,
}

/// The catalog Transaction for a batch Filer has just filed. Doc type, source and tax year come
/// from [`Hint`]s when Vibe Files knew them; otherwise the year is read from the path like
/// `rebuild` does. Returns `None` when the batch wrote nothing.
pub fn record_batch(
    conn: &mut Connection,
    t: &CatalogTarget,
    lb: &LastBatch,
) -> vf_core::Result<Option<Committed>> {
    let root = &t.root;
    let now = vf_core::now_rfc3339();
    let dev = t.actor.device_id.clone();
    let clients = ext_map(conn)?;
    let hints = t.hints.lock().map(|h| h.clone()).unwrap_or_default();

    let mut written: Vec<Written<'_>> = Vec::new();
    for it in &lb.items {
        written.push(Written {
            action: "file",
            src: Some(&it.src),
            dst: &it.dst,
            client_ext: &it.client_id,
        });
        for c in &it.copies {
            written.push(Written {
                action: "copy",
                src: Some(&it.src),
                dst: &c.dst,
                client_ext: &c.client_id,
            });
        }
    }
    for z in &lb.zip_entries {
        written.push(Written {
            action: "unzip",
            src: None,
            dst: &z.dst,
            client_ext: &z.client_id,
        });
    }

    let mut tx = Transaction::new(&format!("intake batch {}", lb.batch));
    let mut new_ids = Vec::new();
    let mut replaced = Vec::new();
    for w in &written {
        if !w.dst.is_file() {
            continue; // Filer reported it, but it is not there (failed later in the batch)
        }
        let rel_path = rel(root, w.dst);
        let hash = sha256_file(w.dst).map_err(|e| vf_core::Error::Tx(e.to_string()))?;
        let size = std::fs::metadata(w.dst).map(|m| m.len()).unwrap_or(0) as i64;
        let client = clients.get(w.client_ext.trim()).cloned();
        let hint = w
            .src
            .and_then(|s| hints.get(s))
            .cloned()
            .unwrap_or_default();
        // Unknown origin stays NULL; the inspector can set it.
        let source = hint.source.clone();
        let year = hint.tax_year.or_else(|| year_from_path(&rel_path));
        let doc_type = hint.doc_type.as_deref().and_then(|n| doc_type_id(conn, n));
        let existing: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT id, hash, size FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                [&rel_path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let id = match existing {
            Some((id, old_hash, old_size)) => {
                // Replace (Overwrite): Filer kept the old file under data/overwritten; keep it
                // as a version too, content-addressed in the cabinet (Q-17).
                if old_hash != hash {
                    let kept = lb
                        .backups
                        .iter()
                        .find(|b| b.original == w.dst)
                        .map(|b| b.kept.clone());
                    let vpath = cabinet::version_path(root, &old_hash);
                    if let Some(kept) = kept
                        && !vpath.exists()
                    {
                        tx = tx.file(FileOp::Copy {
                            src: kept,
                            dst: vpath.clone(),
                        });
                        let sidecar = VersionSidecar {
                            rel_path: rel_path.clone(),
                            created: now.clone(),
                            created_by: t.actor.user.clone(),
                            device_id: dev.clone(),
                            reason: Some("replaced at intake".into()),
                        };
                        tx = tx.file(FileOp::WriteNew {
                            path: vpath.with_extension("json"),
                            contents: serde_json::to_string_pretty(&[sidecar]).expect("json"),
                        });
                    }
                    tx = tx.catalog(Stmt::new(
                        "INSERT OR IGNORE INTO version (doc_id, hash, size, created, created_by, device_id, reason)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'replaced at intake')",
                        vec![id.clone().into(), old_hash.into(), old_size.into(), now.clone().into(), t.actor.user.clone().into(), dev.clone().into()],
                    ));
                }
                tx = tx.catalog(Stmt::new(
                    "UPDATE document SET hash = ?2, size = ?3, filed_by = ?4, device_id = ?5, updated_at = ?6, rev = rev + 1 WHERE id = ?1",
                    vec![id.clone().into(), hash.clone().into(), size.into(), t.actor.user.clone().into(), dev.clone().into(), now.clone().into()],
                ));
                replaced.push(id.clone());
                id
            }
            None => {
                let id = vf_core::new_id();
                tx = tx
                    .catalog(Stmt::new(
                        "INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash, size, mime, source, status, filed_by, filed_at, device_id, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'received', ?10, ?11, ?12, ?11)",
                        vec![
                            id.clone().into(), client.into(), doc_type.into(), year.into(), rel_path.clone().into(),
                            hash.clone().into(), size.into(), mime(&rel_path).into(), source.into(),
                            t.actor.user.clone().into(), now.clone().into(), dev.clone().into(),
                        ],
                    ))
                    .catalog_undo(Stmt::new(
                        "UPDATE document SET deleted_at = ?2, tombstone_reason = 'intake undone', rev = rev + 1 WHERE id = ?1",
                        vec![id.clone().into(), now.clone().into()],
                    ));
                new_ids.push(id.clone());
                id
            }
        };
        tx = tx
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'put', ?2) ON CONFLICT DO NOTHING",
                vec![id.clone().into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                w.action,
                Some(&id),
                json!({
                    "batch": lb.batch.to_string(),
                    "src": w.src.map(|s| s.display().to_string()),
                    "dst": rel_path, "hash": hash,
                }),
            ));
    }
    // Client folders the batch created or bound.
    let mut bound = 0;
    for f in &lb.created_files {
        let Some(folder) = f.parent().and_then(Path::parent) else {
            continue;
        };
        if let Ok(Some(s)) = sentinel::read_sentinel(folder) {
            let h = sha256_file(f).map_err(|e| vf_core::Error::Tx(e.to_string()))?;
            tx = tx.catalog(Stmt::new(
                "UPDATE client SET folder_rel = ?2, sentinel_hash = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1
                 WHERE id = ?1 AND (folder_rel IS NOT ?2 OR sentinel_hash IS NOT ?3)",
                vec![s.client_id.to_string().into(), rel(root, folder).into(), h.into(), dev.clone().into(), now.clone().into()],
            ));
            bound += 1;
        }
    }
    if new_ids.is_empty() && replaced.is_empty() && bound == 0 {
        return Ok(None);
    }
    tx = tx
        .audit(AuditEntry::new(
            "intake_batch",
            None,
            json!({"batch": lb.batch.to_string(), "new": new_ids.len(), "replaced": replaced.len(), "folders": bound}),
        ))
        .extra(json!({"filer_batch": lb.batch.to_string(), "docs": new_ids, "replaced": replaced}));
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| vf_core::Error::Tx(e.to_string()))?;
    Ok(Some(tx.commit(conn, &mut fs, &t.actor)?))
}

/// After Filer undid `lb`: make the catalog match the disk for that batch.
pub fn reconcile_undo(
    conn: &mut Connection,
    t: &CatalogTarget,
    lb: &LastBatch,
) -> vf_core::Result<Option<Committed>> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT tx_id, record FROM undo_record
             WHERE json_extract(record, '$.extra.filer_batch') = ?1 AND undone = 0",
            [lb.batch.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((tx_id, record)) = found else {
        return Ok(None);
    };
    let record: serde_json::Value = serde_json::from_str(&record).unwrap_or_default();
    let ids = |k: &str| -> Vec<String> {
        record["extra"][k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let now = vf_core::now_rfc3339();
    let dev = t.actor.device_id.clone();
    let mut tx = Transaction::new(&format!("intake undo {}", lb.batch)).catalog(Stmt::new(
        "UPDATE undo_record SET undone = 1 WHERE tx_id = ?1",
        vec![tx_id.clone().into()],
    ));
    let mut back = 0;
    for id in ids("docs") {
        let rel_path: String =
            conn.query_row("SELECT rel_path FROM document WHERE id = ?1", [&id], |r| {
                r.get(0)
            })?;
        if !t.root.join(&rel_path).is_file() {
            tx = tx
                .catalog(Stmt::new(
                    "UPDATE document SET deleted_at = ?2, tombstone_reason = 'intake undone', device_id = ?3, updated_at = ?2, rev = rev + 1 WHERE id = ?1",
                    vec![id.clone().into(), now.clone().into(), dev.clone().into()],
                ))
                .catalog(Stmt::new(
                    "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'delete', ?2) ON CONFLICT DO NOTHING",
                    vec![id.clone().into(), now.clone().into()],
                ))
                .audit(AuditEntry::new("intake_undo", Some(&id), json!({"batch": lb.batch.to_string(), "rel_path": rel_path})));
            back += 1;
        }
    }
    for id in ids("replaced") {
        let rel_path: String =
            conn.query_row("SELECT rel_path FROM document WHERE id = ?1", [&id], |r| {
                r.get(0)
            })?;
        let p = t.root.join(&rel_path);
        if let Ok(h) = sha256_file(&p) {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) as i64;
            tx = tx
                .catalog(Stmt::new(
                    "UPDATE document SET hash = ?2, size = ?3, device_id = ?4, updated_at = ?5, rev = rev + 1 WHERE id = ?1 AND hash != ?2",
                    vec![id.clone().into(), h.into(), size.into(), dev.clone().into(), now.clone().into()],
                ))
                .audit(AuditEntry::new("intake_undo", Some(&id), json!({"batch": lb.batch.to_string(), "restored": rel_path})));
        }
    }
    // Client folders the undo removed.
    let mut gone = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT id, folder_rel FROM client WHERE folder_rel IS NOT NULL AND deleted_at IS NULL",
        )?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, f) = r?;
            if !sentinel::sentinel_path(&t.root.join(&f)).is_file() {
                gone.push(id);
            }
        }
    }
    for id in gone {
        tx = tx.catalog(Stmt::new(
            "UPDATE client SET folder_rel = NULL, sentinel_hash = NULL, device_id = ?2, updated_at = ?3, rev = rev + 1 WHERE id = ?1",
            vec![id.into(), dev.clone().into(), now.clone().into()],
        ));
    }
    tx = tx.audit(AuditEntry::new(
        "intake_batch_undo",
        None,
        json!({"batch": lb.batch.to_string(), "tx": tx_id, "back": back}),
    ));
    Ok(Some(tx.commit(conn, &mut NoFileOps, &t.actor)?))
}

fn catalog_error_outcome(e: &str) -> String {
    format!("filed, but the catalog could not record it ({e}); the file was put back")
}

/// Filer hooks that record every batch in the catalog (see the module docs).
pub fn hooks(t: CatalogTarget) -> Hooks {
    let tc = t.clone();
    let commit_fn = move |input: &commit::CommitInput, p: &std::sync::atomic::AtomicUsize| {
        let mut report = commit::run_commit(input, p);
        if report.last_batch.items.is_empty() {
            return report;
        }
        let recorded = tc
            .open()
            .and_then(|mut c| record_batch(&mut c, &tc, &report.last_batch));
        if let Err(e) = recorded {
            let e = e.to_string();
            let u = undo::run_undo(&input.data_dir, report.last_batch.clone(), input.force_copy);
            for o in &mut report.outcomes {
                if o.result.is_ok() {
                    o.result = Err(catalog_error_outcome(&e));
                }
            }
            report.warnings.push(format!(
                "The batch was undone: the catalog could not record it ({e})."
            ));
            report.warnings.extend(u.warnings);
            report.last_batch = u.last_batch;
        }
        report
    };
    let tu = t.clone();
    let unpack_fn = move |input: &commit::UnpackInput, p: &std::sync::atomic::AtomicUsize| {
        let mut report = commit::run_unpack(input, p);
        if report.last_batch.zip_entries.is_empty() {
            return report;
        }
        let recorded = tu
            .open()
            .and_then(|mut c| record_batch(&mut c, &tu, &report.last_batch));
        if let Err(e) = recorded {
            let e = e.to_string();
            let u = undo::run_undo(&input.target.data_dir, report.last_batch.clone(), false);
            report.errors.push(format!(
                "The unpack was undone: the catalog could not record it ({e})."
            ));
            report.warnings.extend(u.warnings);
            report.written = 0;
            report.last_batch = u.last_batch;
        }
        report
    };
    let tn = t;
    let undo_fn = move |data: &Path, lb: LastBatch, force: bool| {
        let mut u = undo::run_undo(data, lb, force);
        if let Err(e) = tn
            .open()
            .and_then(|mut c| reconcile_undo(&mut c, &tn, &u.last_batch))
        {
            u.warnings.push(format!(
                "Files were put back, but the catalog could not follow ({e}); run Rebuild in Admin."
            ));
        }
        u
    };
    Hooks {
        commit: Arc::new(commit_fn),
        unpack: Arc::new(unpack_fn),
        undo: Arc::new(undo_fn),
    }
}

/// The client list Filer matches against, built from the catalog: same CSV IDs, names, extras and
/// uuids (= catalog ids = sentinel `client_id`s).
pub fn filer_client_list(conn: &Connection) -> vf_core::Result<fclients::ClientList> {
    let mut st = conn.prepare(
        "SELECT id, ext_id, name, extra FROM client WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE",
    )?;
    let rows: Vec<(String, String, String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut clients = Vec::new();
    let mut uuids = BTreeMap::new();
    for (id, ext, name, extra) in rows {
        let Ok(u) = Uuid::parse_str(&id) else {
            continue;
        };
        uuids.insert(ext.clone(), u);
        clients.push(Client {
            id: ext,
            uuid: u,
            name,
            extra: serde_json::from_str(&extra).unwrap_or_default(),
        });
    }
    let now = chrono::Utc::now();
    let v = json!({
        "source": "vibe-files catalog",
        "source_modified": now,
        "imported_at": now,
        "column_map": filer_core::config::ColumnMap::default(),
        "skipped_rows": 0,
        "clients": clients,
        "uuids": uuids,
    });
    serde_json::from_value(v).map_err(|e| vf_core::Error::Tx(format!("client list: {e}")))
}

/// Writes Filer's data folder from the cabinet. Relationships added in Filer's own
/// Relationships tab since the last call are merged into the catalog first.
pub fn prepare_filer_dir(
    conn: &mut Connection,
    actor: &Actor,
    filer_dir: &Path,
    root: &Path,
    cab: &CabinetFile,
    tpl: &TemplateNode,
    default_inbox: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(filer_dir)
        .map_err(|e| format!("cannot create {}: {e}", filer_dir.display()))?;
    let (mut s, savable) = Settings::load_or_default(filer_dir);
    if !savable {
        return Err(format!(
            "{} is not readable; move it aside",
            Settings::path(filer_dir).display()
        ));
    }
    s.root = Some(root.to_path_buf());
    s.firm_id = Some(Uuid::parse_str(&cab.firm_id).map_err(|e| format!("cabinet firm id: {e}"))?);
    s.csv_path = None; // the client list comes from the catalog
    if s.inboxes.is_empty() {
        std::fs::create_dir_all(default_inbox).map_err(|e| e.to_string())?;
        s.inboxes.push(default_inbox.to_path_buf());
    }
    s.save(filer_dir).map_err(|e| e.to_string())?;
    template::save(filer_dir, tpl).map_err(|e| e.to_string())?;
    let list = filer_client_list(conn).map_err(|e| e.to_string())?;
    fclients::save_cache(filer_dir, &list).map_err(|e| e.to_string())?;

    // Relationships: additions made in Filer's own Relationships tab → catalog, then
    // catalog → Filer's file.
    let (mut theirs, ok, _) = filer_core::relationships::load(filer_dir);
    let ours = crate::clients::to_filer(conn).map_err(|e| e.to_string())?;
    let key = |r: &filer_core::relationships::Relationship| {
        (
            r.parent_id.trim().to_owned(),
            r.child_id.trim().to_owned(),
            r.role.trim().to_lowercase(),
        )
    };
    let known: std::collections::HashSet<_> = ours.records.iter().map(key).collect();
    theirs.records.retain(|r| !known.contains(&key(r)));
    if ok {
        let map = ext_map(conn).map_err(|e| e.to_string())?;
        let stmts = crate::clients::import_filer_stmts(
            &theirs,
            &map,
            &actor.device_id,
            &vf_core::now_rfc3339(),
        );
        if !stmts.is_empty() {
            let mut tx = Transaction::new("relationships from intake").audit(AuditEntry::new(
                "relationship_import",
                None,
                json!({"from": "intake", "count": stmts.len()}),
            ));
            for st in stmts {
                tx = tx.catalog(st);
            }
            tx.commit(conn, &mut NoFileOps, actor)
                .map_err(|e| e.to_string())?;
            crate::clients::mirror_relationships(conn, root).map_err(|e| e.to_string())?;
        }
    }
    let ours = crate::clients::to_filer(conn).map_err(|e| e.to_string())?;
    filer_core::relationships::save(filer_dir, &ours).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use filer_intake::commit::{CommitInput, CommitItem};
    use filer_intake::intake::Source;
    use filer_intake::plan::FolderAction;

    use super::*;
    use crate::clients::{NewClient, create_client, template as cab_template};

    struct Setup {
        w: crate::clients::tests::World,
        filer: PathBuf,
        inbox: PathBuf,
        folder: PathBuf,
        client: Client,
        target: CatalogTarget,
    }

    fn setup() -> Setup {
        let mut w = crate::clients::tests::world();
        vf_core::doctype::seed(&mut w.conn, &w.actor).unwrap();
        let tpl = cab_template(&w.root);
        let nc = NewClient {
            ext_id: "1042".into(),
            name: "Anderson Farms LLC".into(),
            ..NewClient::default()
        };
        let (id, _) = create_client(&mut w.conn, &w.root, &w.cab, &tpl, &nc, &w.actor).unwrap();
        let base = w.root.parent().unwrap().to_path_buf();
        let filer = base.join("data/filer");
        let inbox = base.join("Inbox");
        prepare_filer_dir(&mut w.conn, &w.actor, &filer, &w.root, &w.cab, &tpl, &inbox).unwrap();
        let list = fclients::load_cache(&filer).unwrap().unwrap();
        let client = list.match_by_id("1042").unwrap().clone();
        assert_eq!(
            client.uuid.to_string(),
            id,
            "Filer sees the catalog's client id"
        );
        let target = CatalogTarget {
            catalog: base.join("c.db"),
            key: CatalogKey([5; 32]),
            root: w.root.clone(),
            actor: w.actor.clone(),
            hints: Hints::default(),
        };
        Setup {
            folder: w.root.join("Anderson Farms LLC (1042)"),
            w,
            filer,
            inbox,
            client,
            target,
        }
    }

    fn item(s: &Setup, name: &str, overwrite: bool) -> CommitItem {
        CommitItem {
            src: s.inbox.join(name),
            dest: s.folder.join("Income Tax/2025").join(name),
            folder: FolderAction::Exists(s.folder.clone()),
            client: s.client.clone(),
            overwrite,
            source: Source::Inbox(s.inbox.clone()),
            merged_from: vec![],
            split_of: None,
            copies: vec![],
            kept_both: false,
            also_dirs: vec![],
        }
    }

    fn input(s: &Setup, items: Vec<CommitItem>) -> CommitInput {
        CommitInput {
            batch: Uuid::new_v4(),
            items,
            skipped: vec![],
            root: s.w.root.clone(),
            first_inbox: Some(s.inbox.clone()),
            data_dir: s.filer.clone(),
            firm_id: Uuid::parse_str(&s.w.cab.firm_id).unwrap(),
            template: cab_template(&s.w.root),
            splits: vec![],
            force_copy: false,
        }
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// P02 done-when: Undo last batch restores every file and catalog row.
    #[test]
    fn commit_records_catalog_and_undo_restores_files_and_rows() {
        let mut s = setup();
        let year = s.folder.join("Income Tax/2025");
        std::fs::create_dir_all(&year).unwrap();
        std::fs::write(year.join("old.pdf"), b"%PDF old").unwrap();
        crate::rebuild::rebuild(&mut s.w.conn, &s.w.root, "dev").unwrap();
        std::fs::create_dir_all(&s.inbox).unwrap();
        std::fs::write(s.inbox.join("w2.pdf"), b"%PDF w2").unwrap();
        std::fs::write(s.inbox.join("old.pdf"), b"%PDF new").unwrap();
        s.target.hints.lock().unwrap().insert(
            s.inbox.join("w2.pdf"),
            Hint {
                doc_type: Some("W-2".into()),
                source: Some("client".into()),
                tax_year: Some(2025),
            },
        );
        let docs_before = count(
            &s.w.conn,
            "SELECT count(*) FROM document WHERE deleted_at IS NULL",
        );

        let h = hooks(s.target.clone());
        let inp = input(
            &s,
            vec![item(&s, "w2.pdf", false), item(&s, "old.pdf", true)],
        );
        let report = (h.commit)(&inp, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        assert!(year.join("w2.pdf").is_file() && !s.inbox.join("w2.pdf").exists());
        assert_eq!(std::fs::read(year.join("old.pdf")).unwrap(), b"%PDF new");

        let c = &s.w.conn;
        assert_eq!(
            count(c, "SELECT count(*) FROM document WHERE deleted_at IS NULL"),
            docs_before + 1
        );
        let (dt, src, yr): (String, String, i64) = c
            .query_row(
                "SELECT t.name, d.source, d.tax_year FROM document d JOIN doc_type t ON t.id = d.doc_type_id
                 WHERE d.rel_path LIKE '%/w2.pdf'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((dt.as_str(), src.as_str(), yr), ("W-2", "client", 2025));
        assert_eq!(
            count(
                c,
                "SELECT count(*) FROM version WHERE reason = 'replaced at intake'"
            ),
            1
        );
        assert_eq!(
            count(c, "SELECT count(*) FROM audit WHERE action = 'file'"),
            2
        );
        assert!(vf_core::audit::verify(c).unwrap().ok());
        // The overwritten content is kept as a version in the cabinet (Q-17).
        let old_hash: String = c
            .query_row(
                "SELECT hash FROM version WHERE reason = 'replaced at intake'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(cabinet::version_path(&s.w.root, &old_hash)).unwrap(),
            b"%PDF old"
        );

        let u = (h.undo)(&s.filer, report.last_batch.clone(), false);
        assert!(u.outcomes.iter().all(|o| o.1.is_ok()), "{:?}", u.outcomes);
        assert!(s.inbox.join("w2.pdf").is_file() && !year.join("w2.pdf").exists());
        assert_eq!(std::fs::read(year.join("old.pdf")).unwrap(), b"%PDF old");
        let c = &s.w.conn;
        assert_eq!(
            count(c, "SELECT count(*) FROM document WHERE deleted_at IS NULL"),
            docs_before
        );
        assert_eq!(
            count(c, "SELECT count(*) FROM undo_record WHERE undone = 1"),
            1
        );
        // The catalog matches the disk again.
        let rep = crate::rebuild::rebuild(&mut s.w.conn, &s.w.root, "dev").unwrap();
        assert_eq!(
            rep.docs_new + rep.docs_changed + rep.docs_moved + rep.docs_tombstoned,
            0,
            "{rep}"
        );
    }

    #[test]
    fn catalog_failure_puts_the_files_back() {
        let mut s = setup();
        std::fs::create_dir_all(&s.inbox).unwrap();
        std::fs::write(s.inbox.join("a.pdf"), b"%PDF a").unwrap();
        s.target.key = CatalogKey([9; 32]); // wrong key: the catalog step fails
        let h = hooks(s.target.clone());
        let report = (h.commit)(
            &input(&s, vec![item(&s, "a.pdf", false)]),
            &AtomicUsize::new(0),
        );
        assert!(report.outcomes.iter().all(|o| o.result.is_err()));
        assert!(
            s.inbox.join("a.pdf").is_file(),
            "the file is back in the inbox"
        );
        assert!(!s.folder.join("Income Tax/2025/a.pdf").exists());
        assert_eq!(count(&s.w.conn, "SELECT count(*) FROM document"), 0);
        let _ = &mut s;
    }

    #[test]
    fn filer_relationship_additions_merge_into_the_catalog() {
        let mut s = setup();
        let tpl = cab_template(&s.w.root);
        let nc = NewClient {
            ext_id: "1150".into(),
            name: "Delgado, Maria".into(),
            ..NewClient::default()
        };
        create_client(&mut s.w.conn, &s.w.root, &s.w.cab, &tpl, &nc, &s.w.actor).unwrap();
        let mut rels = filer_core::relationships::Relationships::default();
        rels.add(filer_core::relationships::Relationship {
            parent_id: "1042".into(),
            child_id: "1150".into(),
            role: "partner".into(),
            note: "[share 25%]".into(),
        })
        .unwrap();
        filer_core::relationships::save(&s.filer, &rels).unwrap();
        let inbox = s.inbox.clone();
        prepare_filer_dir(
            &mut s.w.conn,
            &s.w.actor,
            &s.filer,
            &s.w.root,
            &s.w.cab,
            &tpl,
            &inbox,
        )
        .unwrap();
        let ours = crate::clients::to_filer(&s.w.conn).unwrap();
        assert_eq!(ours.records.len(), 1);
        assert_eq!(ours.records[0].note, "[share 25%]");
        // Again: nothing new, no extra audit row.
        let audits = count(&s.w.conn, "SELECT count(*) FROM audit");
        prepare_filer_dir(
            &mut s.w.conn,
            &s.w.actor,
            &s.filer,
            &s.w.root,
            &s.w.cab,
            &tpl,
            &inbox,
        )
        .unwrap();
        let last: String =
            s.w.conn
                .query_row(
                    "SELECT action || ' ' || detail FROM audit ORDER BY seq DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(
            count(&s.w.conn, "SELECT count(*) FROM audit"),
            audits,
            "{last}"
        );
    }
}
