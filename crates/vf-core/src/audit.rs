//! Hash-chained, append-only audit trail (CLAUDE.md §Security rules):
//! `{seq, ts, user, device, action, doc, detail, prev_hash, hash}`.
//!
//! `hash = sha256(json(row without hash))`, where `detail` is hashed as the exact JSON text
//! stored, and the first row's `prev_hash` is 64 zeros. Rows are mirrored to
//! `<cabinet>/_Vibe/audit/<yyyy-mm>.jsonl` after commit.

use std::io::Write;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// What a caller asks to record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub action: String,
    pub doc_id: Option<String>,
    pub detail: serde_json::Value,
}

impl AuditEntry {
    pub fn new(action: &str, doc_id: Option<&str>, detail: serde_json::Value) -> Self {
        Self {
            action: action.into(),
            doc_id: doc_id.map(Into::into),
            detail,
        }
    }
}

/// A stored row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRow {
    pub seq: u64,
    pub ts: String,
    pub user: Option<String>,
    pub device: String,
    pub action: String,
    pub doc: Option<String>,
    /// JSON text exactly as stored and hashed.
    pub detail: String,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Serialize)]
struct Hashed<'a> {
    seq: u64,
    ts: &'a str,
    user: Option<&'a str>,
    device: &'a str,
    action: &'a str,
    doc: Option<&'a str>,
    detail: &'a str,
    prev_hash: &'a str,
}

impl AuditRow {
    pub fn compute_hash(&self) -> String {
        let h = Hashed {
            seq: self.seq,
            ts: &self.ts,
            user: self.user.as_deref(),
            device: &self.device,
            action: &self.action,
            doc: self.doc.as_deref(),
            detail: &self.detail,
            prev_hash: &self.prev_hash,
        };
        let json = serde_json::to_vec(&h).expect("audit row serializes");
        hex(&Sha256::digest(json))
    }

    /// `yyyy-mm` of `ts` (RFC 3339), the JSONL file the row belongs in.
    pub fn month(&self) -> &str {
        self.ts.get(..7).unwrap_or("unknown")
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Appends one entry inside the caller's SQLite transaction. Returns the stored row.
pub fn append(
    conn: &Connection,
    ts: &str,
    user: Option<&str>,
    device: &str,
    entry: &AuditEntry,
) -> Result<AuditRow> {
    let last: Option<(u64, String)> = conn
        .query_row(
            "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (seq, prev_hash) = match last {
        Some((s, h)) => (s + 1, h),
        None => (1, GENESIS.to_owned()),
    };
    let mut row = AuditRow {
        seq,
        ts: ts.to_owned(),
        user: user.map(Into::into),
        device: device.to_owned(),
        action: entry.action.clone(),
        doc: entry.doc_id.clone(),
        detail: serde_json::to_string(&entry.detail).expect("detail serializes"),
        prev_hash,
        hash: String::new(),
    };
    row.hash = row.compute_hash();
    conn.execute(
        "INSERT INTO audit (seq, ts, user, device_id, action, doc_id, detail, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            row.seq,
            row.ts,
            row.user,
            row.device,
            row.action,
            row.doc,
            row.detail,
            row.prev_hash,
            row.hash
        ],
    )?;
    Ok(row)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub rows: u64,
    /// First broken row and why, if any.
    pub broken: Option<(u64, String)>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.broken.is_none()
    }
}

/// Walks the whole chain: contiguous `seq` from 1, each `prev_hash` equal to the previous row's
/// `hash`, and each `hash` recomputed from the row.
pub fn verify(conn: &Connection) -> Result<VerifyReport> {
    let mut st = conn.prepare(
        "SELECT seq, ts, user, device_id, action, doc_id, detail, prev_hash, hash
         FROM audit ORDER BY seq",
    )?;
    let rows = st.query_map([], |r| {
        Ok(AuditRow {
            seq: r.get(0)?,
            ts: r.get(1)?,
            user: r.get(2)?,
            device: r.get(3)?,
            action: r.get(4)?,
            doc: r.get(5)?,
            detail: r.get(6)?,
            prev_hash: r.get(7)?,
            hash: r.get(8)?,
        })
    })?;
    let mut expect_prev = GENESIS.to_owned();
    let mut n = 0u64;
    for row in rows {
        let row = row?;
        n += 1;
        let broken = if row.seq != n {
            Some(format!("expected seq {n}, found {}", row.seq))
        } else if row.prev_hash != expect_prev {
            Some("prev_hash does not match the previous row".to_owned())
        } else if row.compute_hash() != row.hash {
            Some("hash does not match the row's content".to_owned())
        } else {
            None
        };
        if let Some(why) = broken {
            return Ok(VerifyReport {
                rows: n,
                broken: Some((row.seq, why)),
            });
        }
        expect_prev = row.hash;
    }
    Ok(VerifyReport {
        rows: n,
        broken: None,
    })
}

/// Appends rows to `<dir>/<yyyy-mm>.jsonl` (one JSON object per line, `detail` as JSON).
pub fn mirror_append(dir: &Path, rows: &[AuditRow]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for row in rows {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}.jsonl", row.month())))?;
        let detail: serde_json::Value =
            serde_json::from_str(&row.detail).unwrap_or(serde_json::Value::Null);
        let line = serde_json::json!({
            "seq": row.seq, "ts": row.ts, "user": row.user, "device": row.device,
            "action": row.action, "doc": row.doc, "detail": detail,
            "detail_text": row.detail, "prev_hash": row.prev_hash, "hash": row.hash,
        });
        writeln!(f, "{line}")?;
        f.sync_data()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CatalogKey, Storage, open};

    fn conn(dir: &Path) -> Connection {
        open(&dir.join("c.db"), &CatalogKey([3; 32]), Storage::Local).unwrap()
    }

    fn fill(conn: &Connection, n: u64) {
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..n {
            let e = AuditEntry::new(
                "file",
                Some(&format!("doc-{i}")),
                serde_json::json!({"i": i, "note": "synthetic"}),
            );
            append(&tx, "2026-11-02T12:00:00Z", Some("kwk"), "dev-1", &e).unwrap();
        }
        tx.commit().unwrap();
    }

    /// P00 done-when: verify passes after 1,000 synthetic rows and fails after one is tampered.
    #[test]
    fn chain_of_1000_verifies_and_tamper_is_caught() {
        let tmp = tempfile::tempdir().unwrap();
        let c = conn(tmp.path());
        fill(&c, 1000);
        let rep = verify(&c).unwrap();
        assert_eq!(rep.rows, 1000);
        assert!(rep.ok(), "{rep:?}");

        // Someone with raw database access drops the guard and edits row 500.
        c.execute_batch("DROP TRIGGER audit_no_update").unwrap();
        c.execute(
            "UPDATE audit SET detail = '{\"i\":500,\"note\":\"edited\"}' WHERE seq = 500",
            [],
        )
        .unwrap();
        let rep = verify(&c).unwrap();
        assert_eq!(rep.broken.as_ref().map(|b| b.0), Some(500), "{rep:?}");
    }

    #[test]
    fn deleted_row_and_relinked_chain_are_caught() {
        let tmp = tempfile::tempdir().unwrap();
        let c = conn(tmp.path());
        fill(&c, 10);
        c.execute_batch("DROP TRIGGER audit_no_delete").unwrap();
        c.execute("DELETE FROM audit WHERE seq = 4", []).unwrap();
        assert_eq!(verify(&c).unwrap().broken.unwrap().0, 5);
    }

    #[test]
    fn mirror_writes_monthly_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let c = conn(tmp.path());
        let e = AuditEntry::new("login", None, serde_json::json!({}));
        let r1 = append(&c, "2026-10-31T23:59:59Z", None, "d", &e).unwrap();
        let r2 = append(&c, "2026-11-01T00:00:00Z", None, "d", &e).unwrap();
        let dir = tmp.path().join("audit");
        mirror_append(&dir, &[r1, r2]).unwrap();
        assert!(dir.join("2026-10.jsonl").is_file());
        assert!(dir.join("2026-11.jsonl").is_file());
    }
}
