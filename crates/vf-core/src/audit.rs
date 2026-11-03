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

/// `local_state` key: the last `seq` written to the JSONL mirror.
const MIRRORED: &str = "audit_mirrored_seq";

/// Appends to `<dir>/<yyyy-mm>.jsonl` every row not mirrored yet, and remembers how far it got.
/// Run after changes (the app calls it every few seconds); returns how many rows it wrote.
pub fn mirror_pending(conn: &Connection, dir: &Path) -> Result<usize> {
    let done: u64 = conn
        .query_row("SELECT v FROM local_state WHERE k = ?1", [MIRRORED], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let last: u64 = conn.query_row("SELECT coalesce(max(seq), 0) FROM audit", [], |r| r.get(0))?;
    if last <= done {
        return Ok(0);
    }
    let mut st = conn.prepare(
        "SELECT seq, ts, user, device_id, action, doc_id, detail, prev_hash, hash
         FROM audit WHERE seq > ?1 ORDER BY seq",
    )?;
    let rows = st
        .query_map([done], |r| {
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
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    mirror_append(dir, &rows).map_err(|e| crate::Error::Tx(format!("audit mirror: {e}")))?;
    conn.execute(
        "INSERT INTO local_state (k, v) VALUES (?1, ?2)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![MIRRORED, last.to_string()],
    )?;
    Ok(rows.len())
}

/// What the Audit trail page shows (PHASES P05 step 2). Empty fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Part of the Windows login (or device id when no login was recorded).
    pub user: String,
    /// Client uuid: rows about its documents or about the client itself.
    pub client_id: Option<String>,
    /// Part of the document's cabinet path.
    pub doc: String,
    pub action: Option<String>,
    /// `yyyy-mm-dd` (UTC date of the row), inclusive.
    pub from: String,
    pub to: String,
}

/// An audit row with what it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub row: AuditRow,
    /// The document's cabinet path (now), if the row is about one.
    pub doc_path: Option<String>,
    /// The client's name, from the document or the row's detail.
    pub client: Option<String>,
}

/// Rows matching `f`, oldest first (by `seq`: the order they happened in). `limit`: the
/// newest that many.
pub fn query(conn: &Connection, f: &Filter, limit: Option<usize>) -> Result<Vec<Listed>> {
    let sql = "SELECT a.seq, a.ts, a.user, a.device_id, a.action, a.doc_id, a.detail,
                      a.prev_hash, a.hash, d.rel_path,
                      coalesce(c.name, c2.name)
               FROM audit a
               LEFT JOIN document d ON d.id = a.doc_id
               LEFT JOIN client c ON c.id = d.client_id
               LEFT JOIN client c2 ON c2.deleted_at IS NULL AND (
                    c2.id = coalesce(json_extract(a.detail, '$.client'),
                                     json_extract(a.detail, '$.client_id'))
                    OR c2.ext_id = json_extract(a.detail, '$.client'))
               WHERE (?1 = '' OR coalesce(a.user, a.device_id) LIKE '%' || ?1 || '%')
                 AND (?2 IS NULL OR d.client_id = ?2
                      OR json_extract(a.detail, '$.client_id') = ?2
                      OR json_extract(a.detail, '$.client')
                         IN (?2, (SELECT ext_id FROM client WHERE id = ?2)))
                 AND (?3 = '' OR d.rel_path LIKE '%' || ?3 || '%')
                 AND (?4 IS NULL OR a.action = ?4)
                 AND (?5 = '' OR substr(a.ts, 1, 10) >= ?5)
                 AND (?6 = '' OR substr(a.ts, 1, 10) <= ?6)
               ORDER BY a.seq DESC
               LIMIT ?7";
    let mut st = conn.prepare(sql)?;
    let lim = limit.map_or(-1, |l| l as i64);
    let rows = st.query_map(
        params![
            f.user.trim(),
            f.client_id,
            f.doc.trim(),
            f.action,
            f.from.trim(),
            f.to.trim(),
            lim
        ],
        |r| {
            Ok(Listed {
                row: AuditRow {
                    seq: r.get(0)?,
                    ts: r.get(1)?,
                    user: r.get(2)?,
                    device: r.get(3)?,
                    action: r.get(4)?,
                    doc: r.get(5)?,
                    detail: r.get(6)?,
                    prev_hash: r.get(7)?,
                    hash: r.get(8)?,
                },
                doc_path: r.get(9)?,
                client: r.get(10)?,
            })
        },
    )?;
    let mut out = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    out.reverse();
    Ok(out)
}

/// The actions and the users (logins, else devices) that occur, sorted, for the filters.
pub fn choices(conn: &Connection) -> Result<(Vec<String>, Vec<String>)> {
    let list = |sql: &str| -> Result<Vec<String>> {
        let mut st = conn.prepare(sql)?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(v)
    };
    Ok((
        list("SELECT DISTINCT action FROM audit ORDER BY action")?,
        list("SELECT DISTINCT coalesce(user, device_id) FROM audit ORDER BY 1")?,
    ))
}

/// The columns of [`to_csv`] and the PDF export.
pub const COLUMNS: [&str; 9] = [
    "seq",
    "time (UTC)",
    "user",
    "device",
    "action",
    "client",
    "document",
    "detail",
    "hash",
];

/// One row as the export's cells.
pub fn cells(l: &Listed) -> [String; 9] {
    let r = &l.row;
    [
        r.seq.to_string(),
        r.ts.clone(),
        r.user.clone().unwrap_or_default(),
        r.device.clone(),
        r.action.clone(),
        l.client.clone().unwrap_or_default(),
        l.doc_path
            .clone()
            .or_else(|| r.doc.clone())
            .unwrap_or_default(),
        r.detail.clone(),
        r.hash.clone(),
    ]
}

/// The rows as CSV (header first), in the order given.
pub fn to_csv(rows: &[Listed]) -> String {
    let mut w = csv::Writer::from_writer(Vec::new());
    let _ = w.write_record(COLUMNS);
    for l in rows {
        let _ = w.write_record(cells(l));
    }
    String::from_utf8(w.into_inner().unwrap_or_default()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Storage, open};

    fn conn(dir: &Path) -> Connection {
        open(&dir.join("c.db"), Storage::Local).unwrap()
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

    /// PHASES P05 done-when: an export for a client reconstructs its full history in order.
    #[test]
    fn client_export_has_its_whole_history_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let c = conn(tmp.path());
        c.execute_batch(
            "INSERT INTO client (id, ext_id, name, device_id, updated_at)
               VALUES ('c1', 'ANDE1042', 'Anderson Farms LLC', 'd', 't'),
                      ('c2', 'BELL2002', 'Bell Ranch', 'd', 't');
             INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at)
               VALUES ('a', 'c1', 'Anderson Farms LLC (ANDE1042)/w2.pdf', 'h', 1, 't', 'd', 't'),
                      ('b', 'c2', 'Bell Ranch (BELL2002)/k1.pdf', 'h2', 1, 't', 'd', 't');",
        )
        .unwrap();
        let add = |ts: &str, action: &str, doc: Option<&str>, detail: serde_json::Value| {
            append(
                &c,
                ts,
                Some("OFFICE\\ann"),
                "dev",
                &AuditEntry::new(action, doc, detail),
            )
            .unwrap();
        };
        add(
            "2026-01-02T10:00:00Z",
            "client_create",
            None,
            // Client actions and Intake's ops name the client by its CSV ID.
            serde_json::json!({"client": "ANDE1042"}),
        );
        add(
            "2026-01-03T10:00:00Z",
            "file",
            Some("a"),
            serde_json::json!({}),
        );
        add(
            "2026-01-03T11:00:00Z",
            "file",
            Some("b"),
            serde_json::json!({}),
        );
        add(
            "2026-02-01T09:00:00Z",
            "meta_edit",
            Some("a"),
            serde_json::json!({"status": "final"}),
        );
        add(
            "2026-03-01T09:00:00Z",
            "annotate",
            Some("a"),
            serde_json::json!({}),
        );
        add(
            "2026-03-02T09:00:00Z",
            "trash",
            Some("b"),
            serde_json::json!({}),
        );

        let f = Filter {
            client_id: Some("c1".into()),
            ..Filter::default()
        };
        let rows = query(&c, &f, None).unwrap();
        let actions: Vec<&str> = rows.iter().map(|l| l.row.action.as_str()).collect();
        assert_eq!(actions, ["client_create", "file", "meta_edit", "annotate"]);
        assert!(
            rows.windows(2).all(|w| w[0].row.seq < w[1].row.seq),
            "in order"
        );
        assert!(
            rows.iter()
                .all(|l| l.client.as_deref() == Some("Anderson Farms LLC"))
        );
        let csv = to_csv(&rows);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("seq,time (UTC),user"));
        assert!(lines[2].contains("Anderson Farms LLC (ANDE1042)/w2.pdf"));
        assert!(
            lines[3].contains(r#""{""status"":""final""}""#),
            "{}",
            lines[3]
        );

        // Other filters.
        let by = |f: Filter| query(&c, &f, None).unwrap().len();
        assert_eq!(
            by(Filter {
                action: Some("file".into()),
                ..Filter::default()
            }),
            2
        );
        assert_eq!(
            by(Filter {
                doc: "k1".into(),
                ..Filter::default()
            }),
            2
        );
        assert_eq!(
            by(Filter {
                from: "2026-02-01".into(),
                to: "2026-03-01".into(),
                ..Filter::default()
            }),
            2
        );
        assert_eq!(
            by(Filter {
                user: "ann".into(),
                ..Filter::default()
            }),
            6
        );
        assert_eq!(
            by(Filter {
                user: "bob".into(),
                ..Filter::default()
            }),
            0
        );
        let newest = query(&c, &Filter::default(), Some(2)).unwrap();
        assert_eq!(newest.iter().map(|l| l.row.seq).collect::<Vec<_>>(), [5, 6]);
        let (actions, users) = choices(&c).unwrap();
        assert_eq!(actions.len(), 5);
        assert_eq!(users, ["OFFICE\\ann"]);
    }

    /// Rows reach the JSONL mirror once each, however often it runs.
    #[test]
    fn mirror_pending_writes_each_row_once() {
        let tmp = tempfile::tempdir().unwrap();
        let c = conn(tmp.path());
        let dir = tmp.path().join("audit");
        fill(&c, 3);
        assert_eq!(mirror_pending(&c, &dir).unwrap(), 3);
        assert_eq!(mirror_pending(&c, &dir).unwrap(), 0);
        fill(&c, 2);
        assert_eq!(mirror_pending(&c, &dir).unwrap(), 2);
        let text = std::fs::read_to_string(dir.join("2026-11.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 5);
        let seqs: Vec<u64> = text
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["seq"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(seqs, [1, 2, 3, 4, 5]);
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
