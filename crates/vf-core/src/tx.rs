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
//! If the SQLite commit itself fails, the file ops are reversed too.
//!
//! Crash safety (P13): before step 1, a transaction with file ops writes its plan to
//! `tx_journal` (committed on its own); step 3 deletes that row in the same SQLite commit. A row
//! left by a killed process is reversed at the next open ([`journal`];
//! `vf_fs::fileops::recover`). Statements are stored as SQL
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
    /// Remove one empty folder (never one holding anything). Undo creates it again.
    Rmdir { path: PathBuf },
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

/// One planned file op in `tx_journal`, with whether its target existed before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalOp {
    pub op: FileOp,
    pub existed: bool,
    /// The op is being undone (its reversal is what may have landed): recovery applies it again.
    #[serde(default)]
    pub undo: bool,
}

/// A transaction that started changing files and never committed (the process died).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub tx_id: String,
    pub ts: String,
    pub label: String,
    pub ops: Vec<JournalOp>,
    /// The running copy that wrote it ([`set_journal_owner`]); `None` before PA-34.
    pub owner: Option<String>,
}

/// This process's id in `tx_journal.owner` (set when a cabinet opens).
static JOURNAL_OWNER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Marks the journal rows this process writes from now on as its own.
pub fn set_journal_owner(owner: Option<String>) {
    if let Ok(mut o) = JOURNAL_OWNER.lock() {
        *o = owner;
    }
}

/// This process's journal owner id.
pub fn journal_owner() -> Option<String> {
    JOURNAL_OWNER.lock().ok().and_then(|o| o.clone())
}

impl FileOp {
    /// The path this op creates or changes.
    pub fn target(&self) -> PathBuf {
        match self {
            FileOp::Move { dst, .. } | FileOp::Copy { dst, .. } => dst.clone(),
            FileOp::Mkdir { path } | FileOp::Rmdir { path } | FileOp::AppendLines { path, .. } => {
                path.clone()
            }
            FileOp::WriteSentinel { folder, .. } => folder.join("_Vibe").join("client.json"),
        }
    }
}

/// Transactions killed between their file changes and their catalog commit.
pub fn journal(conn: &Connection) -> Result<Vec<JournalEntry>> {
    let mut st =
        conn.prepare("SELECT tx_id, ts, label, ops, owner FROM tx_journal ORDER BY tx_id")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut out = vec![];
    for r in rows {
        let (tx_id, ts, label, ops, owner) = r?;
        let ops = serde_json::from_str(&ops)
            .map_err(|e| Error::Tx(format!("tx_journal {tx_id}: {e}")))?;
        out.push(JournalEntry {
            tx_id,
            ts,
            label,
            ops,
            owner,
        });
    }
    Ok(out)
}

/// Forgets a journal entry (after recovery).
pub fn clear_journal(conn: &Connection, tx_id: &str) -> Result<()> {
    conn.execute("DELETE FROM tx_journal WHERE tx_id = ?1", [tx_id])?;
    Ok(())
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
    /// free). Undo checks every op first and changes nothing if any is blocked. `vacated`: paths
    /// that ops reversed before this one move away (a file swapped for new content: undoing
    /// the swap moves the new file out before the old one moves back).
    fn check_reverse(
        &self,
        _applied: &AppliedOp,
        _vacated: &[PathBuf],
    ) -> std::result::Result<(), String> {
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
        // Prepared once per connection and reused: a rebuild runs the same few statements
        // hundreds of thousands of times (PA-34).
        Ok(conn
            .prepare_cached(&self.sql)?
            .execute(rusqlite::params_from_iter(vals))?)
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
    /// This workstation and the Windows login running it (Q-29: no sign-in in v1; users
    /// arrive with PA-15).
    pub fn device(device_id: &str) -> Self {
        Actor {
            user: crate::windows_login(),
            device_id: device_id.into(),
        }
    }
}

/// How many transactions the undo stack shows (PHASES P05 step 4).
pub const UNDO_DEPTH: usize = 20;

/// One entry of the undo stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undoable {
    pub tx_id: String,
    pub ts: String,
    pub label: String,
    /// What the audit rows say it did (first row's action and document count).
    pub summary: String,
}

/// The last [`UNDO_DEPTH`] transactions `actor` made that are not undone, newest first,
/// leaving out `except` labels (Intake batches are undone by Filer's own Undo). Per Windows
/// login on any workstation (Q-29); rows written before logins were recorded fall back to
/// this workstation.
pub fn undo_stack(conn: &Connection, actor: &Actor, except: &[&str]) -> Result<Vec<Undoable>> {
    // The labels left out and the depth are applied before the summaries are looked up, and
    // those go by the audit_tx index (PA-34: years of audit rows).
    let marks = vec!["?"; except.len()].join(", ");
    let not_in = if except.is_empty() {
        String::new()
    } else {
        format!(" AND u.label NOT IN ({marks})")
    };
    let mut st = conn.prepare(&format!(
        "SELECT u.tx_id, u.ts, u.label FROM undo_record u
         WHERE u.undone = 0
           AND (u.user = ? OR (u.user IS NULL AND u.device_id = ?)){not_in}
         ORDER BY u.rowid DESC LIMIT {UNDO_DEPTH}"
    ))?;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&actor.user, &actor.device_id];
    params.extend(except.iter().map(|l| l as &dyn rusqlite::ToSql));
    let picked: Vec<(String, String, String)> = st
        .query_map(params.as_slice(), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    // Each summary by its own lookup: SQLite does not use the audit_tx expression index inside
    // a correlated subquery, which read every audit row per entry (scale test: 0.6 s).
    let mut first = conn.prepare_cached(
        "SELECT action FROM audit WHERE json_extract(detail, '$.tx') = ?1 ORDER BY seq LIMIT 1",
    )?;
    let mut docs = conn.prepare_cached(
        "SELECT count(DISTINCT doc_id) FROM audit WHERE json_extract(detail, '$.tx') = ?1",
    )?;
    let mut rows = Vec::with_capacity(picked.len());
    for (tx_id, ts, label) in picked {
        let action: Option<String> =
            rusqlite::OptionalExtension::optional(first.query_row([&tx_id], |r| r.get(0)))?;
        let n: i64 = docs.query_row([&tx_id], |r| r.get(0))?;
        rows.push(Ok::<_, rusqlite::Error>(Undoable {
            tx_id,
            ts,
            label,
            summary: match (action, n) {
                (Some(a), 0) => a,
                (Some(a), 1) => format!("{a} · 1 document"),
                (Some(a), n) => format!("{a} · {n} documents"),
                (None, _) => String::new(),
            },
        }));
    }
    let mut out = Vec::new();
    for r in rows {
        let r = r?;
        if !except.contains(&r.label.as_str()) {
            out.push(r);
            if out.len() == UNDO_DEPTH {
                break;
            }
        }
    }
    Ok(out)
}

/// The newest transaction by `device_id` with one of `labels` that is not undone yet.
pub fn last_undoable(conn: &Connection, device_id: &str, labels: &[&str]) -> Option<String> {
    let marks = vec!["?"; labels.len()].join(", ");
    let sql = format!(
        "SELECT tx_id FROM undo_record WHERE device_id = ? AND undone = 0 AND label IN ({marks})
         ORDER BY rowid DESC LIMIT 1"
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
        let journaled = !self.files.is_empty();
        if journaled {
            // A target an earlier op moves away (a sentinel set aside, then written anew: QA
            // 2026-11-12) is not there when this op runs: recovery may remove what it writes.
            let mut gone: std::collections::HashSet<PathBuf> = Default::default();
            let ops: Vec<JournalOp> = self
                .files
                .iter()
                .map(|op| {
                    let t = op.target();
                    let existed = t.exists() && !gone.contains(&t);
                    if let FileOp::Move { src, dst } = op {
                        gone.insert(src.clone());
                        gone.remove(dst);
                    }
                    JournalOp {
                        existed,
                        op: op.clone(),
                        undo: false,
                    }
                })
                .collect();
            conn.execute(
                "INSERT INTO tx_journal (tx_id, ts, label, ops, owner) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    tx_id,
                    ts,
                    self.label,
                    serde_json::to_string(&ops).expect("journal serializes"),
                    journal_owner()
                ],
            )?;
        }
        let forget = |conn: &Connection| {
            if journaled {
                let _ = clear_journal(conn, &tx_id);
            }
        };
        let sql_tx =
            match rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
                Ok(t) => t,
                Err(e) => {
                    // Nothing was done: no journal row may stay (a later recovery would take it for
                    // an interrupted change and move files back).
                    forget(conn);
                    return Err(e.into());
                }
            };
        let prepared = (|| -> Result<Vec<AuditRow>> {
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
            Ok(rows)
        })();
        let rows = match prepared {
            Ok(r) => r,
            Err(e) => {
                drop(sql_tx);
                forget(conn);
                return Err(e);
            }
        };

        let mut applied: Vec<AppliedOp> = Vec::with_capacity(self.files.len());
        for op in &self.files {
            match fs.apply(op) {
                Ok(a) => applied.push(a),
                Err(e) => {
                    let leftovers = reverse_all(fs, &applied);
                    drop(sql_tx); // rollback
                    forget(conn);
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
            .and_then(|_| sql_tx.execute("DELETE FROM tx_journal WHERE tx_id = ?1", [&tx_id]))
            .map_err(Error::from)
            .and_then(|_| sql_tx.commit().map_err(Error::from));
        if let Err(e) = stored {
            let leftovers = reverse_all(fs, &applied);
            forget(conn);
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
    // Reversal runs newest first: when op i is reversed, the moves after it have already put
    // their files back, vacating their destinations.
    let blocked: Vec<String> = record
        .applied
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let vacated: Vec<PathBuf> = record.applied[i + 1..]
                .iter()
                .filter_map(|later| match &later.op {
                    FileOp::Move { dst, .. } => Some(dst.clone()),
                    _ => None,
                })
                .collect();
            fs.check_reverse(a, &vacated).err()
        })
        .collect();
    if !blocked.is_empty() {
        return Err(Error::UndoBlocked(blocked));
    }
    // Crash safety: the reversal is journaled like a commit (recovery applies the ops again
    // if the undo never committed).
    let journal_id = format!("undo-{}", uuid::Uuid::now_v7());
    let ops: Vec<JournalOp> = record
        .applied
        .iter()
        .map(|a| JournalOp {
            existed: a.op.target().exists(),
            op: a.op.clone(),
            undo: true,
        })
        .collect();
    conn.execute(
        "INSERT INTO tx_journal (tx_id, ts, label, ops, owner) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            journal_id,
            crate::now_rfc3339(),
            format!("undo {label}"),
            serde_json::to_string(&ops).expect("journal serializes"),
            journal_owner(),
        ],
    )?;
    let forget = |conn: &Connection| {
        let _ = clear_journal(conn, &journal_id);
    };
    let sql_tx = match rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
        Ok(t) => t,
        Err(e) => {
            forget(conn);
            return Err(e.into());
        }
    };
    let prepared = (|| -> Result<()> {
        for s in record.catalog_undo.iter().rev() {
            s.run(&sql_tx)?;
        }
        sql_tx.execute(
            "UPDATE undo_record SET undone = 1 WHERE tx_id = ?1",
            [tx_id],
        )?;
        sql_tx.execute("DELETE FROM tx_journal WHERE tx_id = ?1", [&journal_id])?;
        Ok(())
    })();
    if let Err(e) = prepared {
        drop(sql_tx);
        forget(conn);
        return Err(e);
    }
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
    // Newest first; if one fails, the ones already reversed are applied again, so the files
    // stay as the catalog (left as it was) says.
    let mut done: Vec<&AppliedOp> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for a in record.applied.iter().rev() {
        match fs.reverse(a) {
            Ok(()) => done.push(a),
            Err(e) => {
                failed.push(format!("{:?}: {e}", a.op));
                break;
            }
        }
    }
    if !failed.is_empty() {
        for a in done.iter().rev() {
            if let Err(e) = fs.apply(&a.op) {
                failed.push(format!("{:?} could not be put back: {e}", a.op));
            }
        }
        drop(sql_tx);
        forget(conn);
        return Err(Error::FileOp {
            error: "undo could not reverse every file change; the files and the catalog were left as they were"
                .into(),
            not_reversed: failed,
        });
    }
    if let Err(e) = sql_tx.commit() {
        // Put the files back as the catalog still has them.
        for a in &record.applied {
            let _ = fs.apply(&a.op);
        }
        forget(conn);
        return Err(e.into());
    }
    Ok(vec![row])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Storage, open};
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
        let conn = open(&tmp.path().join("c.db"), Storage::Local).unwrap();
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

    /// P05 step 4: each Windows login sees its own last 20, newest first, undone ones gone.
    #[test]
    fn undo_stack_is_per_login_newest_first_and_capped() {
        let (_t, mut c, ann) = setup();
        let bob = Actor {
            user: Some("OFFICE\\bob".into()),
            device_id: "dev-1".into(),
        };
        let edit = |n: usize| {
            Transaction::new("edit").audit(AuditEntry::new(
                "meta_edit",
                Some(&format!("doc-{n}")),
                serde_json::json!({}),
            ))
        };
        let mut ids = Vec::new();
        for n in 0..25 {
            ids.push(edit(n).commit(&mut c, &mut NoFileOps, &ann).unwrap().tx_id);
        }
        edit(99).commit(&mut c, &mut NoFileOps, &bob).unwrap();
        Transaction::new("intake")
            .audit(AuditEntry::new("file", None, serde_json::json!({})))
            .commit(&mut c, &mut NoFileOps, &ann)
            .unwrap();
        let stack = undo_stack(&c, &ann, &["intake"]).unwrap();
        assert_eq!(stack.len(), UNDO_DEPTH);
        assert_eq!(stack[0].tx_id, ids[24]);
        assert_eq!(stack[0].summary, "meta_edit · 1 document");
        undo(&mut c, &mut NoFileOps, &ann, &ids[24]).unwrap();
        assert_eq!(undo_stack(&c, &ann, &["intake"]).unwrap()[0].tx_id, ids[23]);
        let theirs = undo_stack(&c, &bob, &[]).unwrap();
        assert_eq!(theirs.len(), 1);
    }

    #[test]
    fn audit_entry_is_required() {
        let (_t, mut c, actor) = setup();
        let tx = Transaction::new("silent").file(mv("/a", "/b"));
        assert!(tx.commit(&mut c, &mut FakeFs::default(), &actor).is_err());
    }
}
