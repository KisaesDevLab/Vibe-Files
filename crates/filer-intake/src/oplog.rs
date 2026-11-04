//! Append-only operation log, `data/ops.jsonl`, one JSON object per line (CLAUDE.md §6).

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const OPLOG_FILE: &str = "ops.jsonl";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Move,
    CopyVerifyDelete,
    Mkdir,
    Merge,
    Split,
    Undo,
    Skip,
    /// A `_Vibe/client.json` sentinel was written (folder created or bound).
    Sentinel,
    /// A file about to be overwritten was moved to data/overwritten (X74).
    Backup,
    /// A sentinel was renamed to `client.<orphaned|duplicate>-<time>.json` (X123, X124).
    SentinelSetAside,
    /// A real byte copy to a related client, verified, written before the primary move (P07a).
    Copy,
    /// A copy removed again because a later step for the same file failed (P07a).
    Rollback,
    /// A zip was imported (P07b): `src` the zip, `dst` where it went, `note` the summary.
    ZipImport,
    /// One file unpacked from a zip (P07b).
    ZipEntry,
    /// A client folder renamed to the template's name (V9): `src` old, `dst` new path.
    RenameDir,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpResult {
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpRecord {
    pub ts: DateTime<Utc>,
    pub batch: Uuid,
    pub op: Op,
    pub client_id: Option<String>,
    pub src: Option<PathBuf>,
    pub dst: Option<PathBuf>,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
    pub result: OpResult,
    pub error: Option<String>,
    /// For a copy: the client ID of the primary it copies (P07a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_of: Option<String>,
    /// Short summary for zip imports, e.g. "unpack · 118 written · 2 skipped" (P07b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl OpRecord {
    /// A successful `op` of `batch` now, with every optional field empty.
    pub fn new(batch: Uuid, op: Op) -> Self {
        Self {
            ts: Utc::now(),
            batch,
            op,
            client_id: None,
            src: None,
            dst: None,
            bytes: None,
            sha256: None,
            result: OpResult::Ok,
            error: None,
            copy_of: None,
            note: None,
        }
    }
}

/// Lines that could not be parsed are reported, not fatal, so one bad line never hides history.
#[derive(Debug, Default)]
pub struct ReadResult {
    pub records: Vec<OpRecord>,
    /// 1-based line numbers that failed to parse.
    pub bad_lines: Vec<usize>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(OPLOG_FILE)
}

/// Appends one record as a single write and syncs it to disk.
pub fn append(log: &Path, record: &OpRecord) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    let mut f = OpenOptions::new().create(true).append(true).open(log)?;
    f.write_all(line.as_bytes())?;
    f.sync_data()?;
    Ok(())
}

/// Reads every record in file order. A missing log reads as empty.
pub fn read(log: &Path) -> anyhow::Result<ReadResult> {
    let f = match std::fs::File::open(log) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ReadResult::default()),
        Err(e) => return Err(e.into()),
    };
    let mut out = ReadResult::default();
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(r) => out.records.push(r),
            Err(_) => out.bad_lines.push(i + 1),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(op: Op, result: OpResult) -> OpRecord {
        OpRecord {
            ts: "2026-10-30T22:10:05Z".parse().unwrap(),
            batch: Uuid::new_v4(),
            op,
            client_id: Some("1234".into()),
            src: Some(PathBuf::from(r"C:\Inbox\Smith_1234_2025 1040.pdf")),
            dst: Some(PathBuf::from(
                r"\\server\clients\Smith (1234)\Tax Returns\2025\x.pdf",
            )),
            bytes: Some(12345),
            sha256: Some("ab".repeat(32)),
            result,
            error: (result == OpResult::Error).then(|| "access denied".into()),
            copy_of: None,
            note: None,
        }
    }

    #[test]
    fn old_lines_without_copy_of_still_read() {
        // ops.jsonl written before P07a has no copy_of field.
        let line = r#"{"ts":"2026-10-30T22:10:05Z","batch":"5d6c1b1e-6a49-4c47-9c7f-3c4ab8f0d6e1","op":"move","client_id":"1234","src":"a","dst":"b","bytes":1,"sha256":null,"result":"ok","error":null}"#;
        let rec: OpRecord = serde_json::from_str(line).unwrap();
        assert_eq!(rec.copy_of, None);
        assert!(!serde_json::to_string(&rec).unwrap().contains("copy_of"));
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let log = path(dir.path());
        let recs = vec![
            sample(Op::Move, OpResult::Ok),
            sample(Op::CopyVerifyDelete, OpResult::Error),
            sample(Op::Mkdir, OpResult::Ok),
        ];
        for r in &recs {
            append(&log, r).unwrap();
        }
        let got = read(&log).unwrap();
        assert_eq!(got.records, recs);
        assert!(got.bad_lines.is_empty());
    }

    #[test]
    fn wire_format_matches_spec() {
        let v = serde_json::to_value(sample(Op::CopyVerifyDelete, OpResult::Ok)).unwrap();
        assert_eq!(v["op"], "copy_verify_delete");
        assert_eq!(v["result"], "ok");
        assert_eq!(v["ts"], "2026-10-30T22:10:05Z");
        assert!(v["error"].is_null());
    }

    #[test]
    fn missing_log_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let got = read(&path(dir.path())).unwrap();
        assert!(got.records.is_empty() && got.bad_lines.is_empty());
    }

    #[test]
    fn bad_line_is_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let log = path(dir.path());
        append(&log, &sample(Op::Move, OpResult::Ok)).unwrap();
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "{{truncated").unwrap();
        drop(f);
        append(&log, &sample(Op::Skip, OpResult::Ok)).unwrap();
        let got = read(&log).unwrap();
        assert_eq!(got.records.len(), 2);
        assert_eq!(got.bad_lines, vec![2]);
    }
}
