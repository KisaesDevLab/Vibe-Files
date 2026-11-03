//! `Transaction`: the one way anything in the cabinet changes (CLAUDE.md principle 5).
//!
//! A transaction carries file ops, catalog statements, the inverse catalog statements for undo,
//! and audit entries. [`Transaction::commit`]:
//!
//! 1. opens one SQLite transaction and runs the catalog statements, appends the audit rows and
//!    writes the undo record;
//! 2. applies the file ops in order through a [`FileOps`] implementation (in Vibe Files that is
//!    `vf-fs`, which calls the vendored Filer Desktop move/copy primitives);
//! 3. commits SQLite.
//!
//! If a file op fails, the ones already applied are reversed newest first and SQLite rolls back.
//! If the SQLite commit itself fails, the file ops are reversed too. Statements are stored as SQL
//! text + JSON params so the undo record is plain data (per-user undo stack in P05).

use std::path::PathBuf;

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::audit::{self, AuditEntry, AuditRow};
use crate::{Error, Result};

/// A file change. Paths are absolute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    /// Move a file; `dst` must not exist. Missing parent folders are created (and recorded).
    Move { src: PathBuf, dst: PathBuf },
    /// Byte copy, hash-verified; `dst` must not exist. `src` is untouched.
    Copy { src: PathBuf, dst: PathBuf },
    /// Create one folder (and missing parents).
    Mkdir { path: PathBuf },
    /// Append `lines` to an append-only JSONL file (created with its folders if missing).
    /// Undo appends `undo_lines` instead of cutting the file, so the file only ever grows.
    AppendLines {
        path: PathBuf,
        lines: Vec<String>,
        undo_lines: Vec<String>,
    },
    /// Bind `folder` (which must exist, inside `root`) to a client by writing its SentinelV1
    /// `_Vibe/client.json`; never replaces one. Undo removes it only if its bytes are unchanged.
    WriteSentinel {
        root: PathBuf,
        folder: PathBuf,
        /// Client uuid (sentinel `client_id`).
        client_id: String,
        /// CSV / tax-software ID.
        ext_id: String,
        name: String,
        firm_id: String,
    },
}

/// What applying a [`FileOp`] did, with enough detail to reverse it safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedOp {
    pub op: FileOp,
    pub bytes: u64,
    /// SHA-256 of the file written (hex), when known.
    pub sha256: Option<String>,
    /// `move` or `copy_verify_delete` (Filer `Method`), `copy`, or `mkdir`.
    pub method: String,
    /// Folders this op created, outermost first. Reversal removes them only if empty.
    pub created_dirs: Vec<PathBuf>,
}

/// File-system side of a transaction.
pub trait FileOps {
    fn apply(&mut self, op: &FileOp) -> std::result::Result<AppliedOp, String>;
    fn reverse(&mut self, applied: &AppliedOp) -> std::result::Result<(), String>;
    /// Whether `reverse` would succeed now (the file is still as written, its old place is
    /// free). Undo checks every op first and changes nothing if any is blocked.
    fn check_reverse(&self, _applied: &AppliedOp) -> std::result::Result<(), String> {
        Ok(())
    }
}

/// For transactions that change only the catalog (rebuild, metadata edits).
pub struct NoFileOps;

impl FileOps for NoFileOps {
    fn apply(&mut self, op: &FileOp) -> std::result::Result<AppliedOp, String> {
        Err(format!("this transaction may not change files ({op:?})"))
    }
    fn reverse(&mut self, _applied: &AppliedOp) -> std::result::Result<(), String> {
        Ok(())
    }
}

/// A SQL statement with JSON-valued positional params (`?1`, `?2`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stmt {
    pub sql: String,
    pub params: Vec<serde_json::Value>,
}

impl Stmt {
    pub fn new(sql: &str, params: Vec<serde_json::Value>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }

    fn run(&self, conn: &Connection) -> Result<usize> {
        let vals: Vec<rusqlite::types::Value> = self.params.iter().map(to_sql).collect();
        Ok(conn.execute(&self.sql, rusqlite::params_from_iter(vals))?)
    }
}

fn to_sql(v: &serde_json::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        serde_json::Value::Null => V::Null,
        serde_json::Value::Bool(b) => V::Integer(*b as i64),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => V::Integer(i),
            None => V::Real(n.as_f64().unwrap_or_default()),
        },
        serde_json::Value::String(s) => V::Text(s.clone()),
        other => V::Text(other.to_string()),
    }
}

/// Who is acting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub user: Option<String>,
    pub device_id: String,
}

impl Actor {
    /// This workstation, no signed-in user (until P06 adds users).
    pub fn device(device_id: &str) -> Self {
        Actor {
            user: None,
            device_id: device_id.into(),
        }
    }
}

/// The newest transaction by `device_id` with one of `labels` that is not undone yet.
pub fn last_undoable(conn: &Connection, device_id: &str, labels: &[&str]) -> Option<String> {
    let marks = vec!["?"; labels.len()].join(", ");
    let sql = format!(
        "SELECT tx_id FROM undo_record WHERE device_id = ? AND undone = 0 AND label IN ({marks})
         ORDER BY ts DESC, rowid DESC LIMIT 1"
    );
    let params: Vec<&dyn rusqlite::ToSql> = std::iter::once(&device_id as &dyn rusqlite::ToSql)
        .chain(labels.iter().map(|l| l as &dyn rusqlite::ToSql))
        .collect();
    conn.query_row(&sql, params.as_slice(), |r| r.get(0)).ok()
}

/// Stored in `undo_record.record`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoRecord {
    pub applied: Vec<AppliedOp>,
    pub catalog_undo: Vec<Stmt>,
    /// Caller data (e.g. the Filer `LastBatch`) kept for later undo logic.
    pub extra: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct Transaction {
    label: String,
    files: Vec<FileOp>,
    catalog: Vec<Stmt>,
    catalog_undo: Vec<Stmt>,
    audit: Vec<AuditEntry>,
    extra: serde_json::Value,
}

#[derive(Debug)]
pub struct Committed {
    pub tx_id: String,
    pub applied: Vec<AppliedOp>,
    pub audit: Vec<AuditRow>,
}

impl Transaction {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }
    pub fn file(mut self, op: FileOp) -> Self {
        self.files.push(op);
        self
    }
    pub fn catalog(mut self, stmt: Stmt) -> Self {
        self.catalog.push(stmt);
        self
    }
    /// Inverse statements, run newest first on undo.
    pub fn catalog_undo(mut self, stmt: Stmt) -> Self {
        self.catalog_undo.push(stmt);
        self
    }
    pub fn audit(mut self, entry: AuditEntry) -> Self {
        self.audit.push(entry);
        self
    }
    pub fn extra(mut self, extra: serde_json::Value) -> Self {
        self.extra = extra;
        self
    }

    pub fn commit(
        self,
        conn: &mut Connection,
        fs: &mut dyn FileOps,
        actor: &Actor,
    ) -> Result<Committed> {
        if self.audit.is_empty() {
            return Err(Error::Tx("every transaction needs an audit entry".into()));
        }
        let tx_id = uuid::Uuid::now_v7().to_string();
        let ts = crate::now_rfc3339();
        let sql_tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for s in &self.catalog {
            s.run(&sql_tx)?;
        }
        let mut rows = Vec::with_capacity(self.audit.len());
        for e in &self.audit {
            let mut e = e.clone();
            if let serde_json::Value::Object(m) = &mut e.detail {
                m.insert("tx".into(), tx_id.clone().into());
            }
            rows.push(audit::append(
                &sql_tx,
                &ts,
                actor.user.as_deref(),
                &actor.device_id,
                &e,
            )?);
        }

        let mut applied: Vec<AppliedOp> = Vec::with_capacity(self.files.len());
        for op in &self.files {
            match fs.apply(op) {
                Ok(a) => applied.push(a),
                Err(e) => {
                    let leftovers = reverse_all(fs, &applied);
                    drop(sql_tx); // rollback
                    return Err(Error::FileOp {
                        error: e,
                        not_reversed: leftovers,
                    });
                }
            }
        }

        let record = UndoRecord {
            applied: applied.clone(),
            catalog_undo: self.catalog_undo,
            extra: self.extra,
        };
        let stored = sql_tx
            .execute(
                "INSERT INTO undo_record (tx_id, user, device_id, ts, label, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    tx_id,
                    actor.user,
                    actor.device_id,
                    ts,
                    self.label,
                    serde_json::to_string(&record).expect("undo record serializes")
                ],
            )
            .map_err(Error::from)
            .and_then(|_| sql_tx.commit().map_err(Error::from));
        if let Err(e) = stored {
            let leftovers = reverse_all(fs, &applied);
            return Err(Error::FileOp {
                error: format!("catalog commit failed ({e}); file changes reversed"),
                not_reversed: leftovers,
            });
        }
        Ok(Committed {
            tx_id,
            applied,
            audit: rows,
        })
    }
}

/// Reverses newest first; returns descriptions of anything that could not be reversed.
fn reverse_all(fs: &mut dyn FileOps, applied: &[AppliedOp]) -> Vec<String> {
    applied
        .iter()
        .rev()
        .filter_map(|a| fs.reverse(a).err().map(|e| format!("{:?}: {e}", a.op)))
        .collect()
}

/// Undoes a committed transaction as its own audited transaction: file ops reversed newest first
/// (each reversal refuses if the file changed since), then the inverse catalog statements.
/// All-or-nothing on the catalog side; file reversals that fail are reported and the catalog is
/// left untouched.
pub fn undo(
    conn: &mut Connection,
    fs: &mut dyn FileOps,
    actor: &Actor,
    tx_id: &str,
) -> Result<Vec<AuditRow>> {
    let (record, undone, label): (String, bool, String) = conn.query_row(
        "SELECT record, undone, label FROM undo_record WHERE tx_id = ?1",
        [tx_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if undone {
        return Err(Error::Tx(format!("{label} was already undone")));
    }
    let record: UndoRecord =
        serde_json::from_str(&record).map_err(|e| Error::Tx(format!("undo record: {e}")))?;
    let blocked: Vec<String> = record
        .applied
        .iter()
        .filter_map(|a| fs.check_reverse(a).err())
        .collect();
    if !blocked.is_empty() {
        return Err(Error::UndoBlocked(blocked));
    }
    let sql_tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for s in record.catalog_undo.iter().rev() {
        s.run(&sql_tx)?;
    }
    sql_tx.execute(
        "UPDATE undo_record SET undone = 1 WHERE tx_id = ?1",
        [tx_id],
    )?;
    let row = audit::append(
        &sql_tx,
        &crate::now_rfc3339(),
        actor.user.as_deref(),
        &actor.device_id,
        &AuditEntry::new(
            "undo",
            None,
            serde_json::json!({"tx": tx_id, "label": label, "files": record.applied.len()}),
        ),
    )?;
    let failed = reverse_all(fs, &record.applied);
    if !failed.is_empty() {
        drop(sql_tx);
        return Err(Error::FileOp {
            error: "undo could not reverse every file change; the catalog was left as it was"
                .into(),
            not_reversed: failed,
        });
    }
    sql_tx.commit()?;
    Ok(vec![row])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CatalogKey, Storage, open};
    use std::collections::HashSet;

    /// In-memory file system double: a set of paths.
    #[derive(Default)]
    struct FakeFs {
        files: HashSet<PathBuf>,
        fail_on: Option<PathBuf>,
    }

    impl FileOps for FakeFs {
        fn apply(&mut self, op: &FileOp) -> std::result::Result<AppliedOp, String> {
            let FileOp::Move { src, dst } = op else {
                unreachable!()
            };
            if Some(dst) == self.fail_on.as_ref() || self.files.contains(dst) {
                return Err("destination exists".into());
            }
            self.files.remove(src);
            self.files.insert(dst.clone());
            Ok(AppliedOp {
                op: op.clone(),
                bytes: 1,
                sha256: None,
                method: "move".into(),
                created_dirs: vec![],
            })
        }
        fn reverse(&mut self, a: &AppliedOp) -> std::result::Result<(), String> {
            let FileOp::Move { src, dst } = &a.op else {
                unreachable!()
            };
            self.files.remove(dst);
            self.files.insert(src.clone());
            Ok(())
        }
    }

    fn setup() -> (tempfile::TempDir, Connection, Actor) {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(
            &tmp.path().join("c.db"),
            &CatalogKey([9; 32]),
            Storage::Local,
        )
        .unwrap();
        let actor = Actor {
            user: Some("kwk".into()),
            device_id: "dev-1".into(),
        };
        (tmp, conn, actor)
    }

    fn mv(a: &str, b: &str) -> FileOp {
        FileOp::Move {
            src: a.into(),
            dst: b.into(),
        }
    }

    fn tx_two_moves() -> Transaction {
        Transaction::new("file 2")
            .file(mv("/in/a.pdf", "/root/a.pdf"))
            .file(mv("/in/b.pdf", "/root/b.pdf"))
            .catalog(Stmt::new(
                "INSERT INTO device (id, name, first_seen, last_seen, mode) VALUES (?1, ?2, 't', 't', 'local')",
                vec!["dev-1".into(), "PC".into()],
            ))
            .catalog_undo(Stmt::new("DELETE FROM device WHERE id = ?1", vec!["dev-1".into()]))
            .audit(AuditEntry::new("file", None, serde_json::json!({"n": 2})))
    }

    fn count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn commit_applies_everything_and_undo_reverses_it() {
        let (_t, mut c, actor) = setup();
        let mut fs = FakeFs::default();
        fs.files.extend(["/in/a.pdf".into(), "/in/b.pdf".into()]);
        let done = tx_two_moves().commit(&mut c, &mut fs, &actor).unwrap();
        assert!(fs.files.contains(&PathBuf::from("/root/b.pdf")));
        assert_eq!(count(&c, "device"), 1);
        assert_eq!(count(&c, "audit"), 1);
        assert_eq!(count(&c, "undo_record"), 1);

        undo(&mut c, &mut fs, &actor, &done.tx_id).unwrap();
        assert!(fs.files.contains(&PathBuf::from("/in/a.pdf")));
        assert!(!fs.files.contains(&PathBuf::from("/root/a.pdf")));
        assert_eq!(count(&c, "device"), 0);
        assert_eq!(count(&c, "audit"), 2);
        assert!(crate::audit::verify(&c).unwrap().ok());
        assert!(undo(&mut c, &mut fs, &actor, &done.tx_id).is_err());
    }

    #[test]
    fn failed_file_op_reverses_and_rolls_back() {
        let (_t, mut c, actor) = setup();
        let mut fs = FakeFs {
            fail_on: Some("/root/b.pdf".into()),
            ..FakeFs::default()
        };
        fs.files.extend(["/in/a.pdf".into(), "/in/b.pdf".into()]);
        let err = tx_two_moves().commit(&mut c, &mut fs, &actor).unwrap_err();
        assert!(matches!(err, Error::FileOp { ref not_reversed, .. } if not_reversed.is_empty()));
        assert!(fs.files.contains(&PathBuf::from("/in/a.pdf")));
        assert!(!fs.files.contains(&PathBuf::from("/root/a.pdf")));
        assert_eq!(count(&c, "device"), 0);
        assert_eq!(count(&c, "audit"), 0);
        assert_eq!(count(&c, "undo_record"), 0);
    }

    #[test]
    fn failed_catalog_statement_touches_no_files() {
        let (_t, mut c, actor) = setup();
        let mut fs = FakeFs::default();
        fs.files.insert("/in/a.pdf".into());
        let tx = Transaction::new("bad")
            .file(mv("/in/a.pdf", "/root/a.pdf"))
            .catalog(Stmt::new("INSERT INTO nope VALUES (1)", vec![]))
            .audit(AuditEntry::new("file", None, serde_json::json!({})));
        assert!(tx.commit(&mut c, &mut fs, &actor).is_err());
        assert!(fs.files.contains(&PathBuf::from("/in/a.pdf")));
    }

    #[test]
    fn audit_entry_is_required() {
        let (_t, mut c, actor) = setup();
        let tx = Transaction::new("silent").file(mv("/a", "/b"));
        assert!(tx.commit(&mut c, &mut FakeFs::default(), &actor).is_err());
    }
}
