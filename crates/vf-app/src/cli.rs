//! Command-line subcommands (`vibe-files <cmd>`). With no subcommand the app opens its window.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Subcommand;
use vf_core::db::{self, Storage};

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Audit trail tools.
    Audit {
        #[command(subcommand)]
        cmd: AuditCmd,
    },
    /// Rebuild the catalog's client/document/version rows from the cabinet folder tree.
    Rebuild {
        /// Cabinet root folder.
        cabinet: PathBuf,
    },
    /// OCR tools.
    Ocr {
        #[command(subcommand)]
        cmd: OcrCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum OcrCmd {
    /// Read every catalogued PDF that is not searchable yet (an imported cabinet), then write
    /// the text layers. Safe to stop and run again: finished pages are kept. Exit code 0 = all
    /// done, 3 = documents still wait (no OCR engine, or files held), 4 = some failed.
    Backlog {
        /// Cabinet root folder.
        cabinet: PathBuf,
        /// Worker threads (default: cores - 1).
        #[arg(long)]
        threads: Option<usize>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditCmd {
    /// Verify the audit hash chain. Exit code 0 = intact, 2 = broken.
    Verify,
}

/// Runs one subcommand; returns the process exit code.
pub fn run(cmd: &Command, data: &Path, out: &mut dyn Write) -> Result<i32> {
    let catalog = data.join("catalog.db");
    match cmd {
        Command::Audit {
            cmd: AuditCmd::Verify,
        } => {
            if !catalog.is_file() {
                bail!("no catalog at {}", catalog.display());
            }
            let conn = db::open(&catalog, Storage::Local)?;
            let rep = vf_core::audit::verify(&conn)?;
            match rep.broken {
                None => {
                    writeln!(out, "audit chain intact: {} rows", rep.rows)?;
                    Ok(0)
                }
                Some((seq, why)) => {
                    writeln!(out, "audit chain BROKEN at seq {seq}: {why}")?;
                    Ok(2)
                }
            }
        }
        Command::Rebuild { cabinet } => {
            std::fs::create_dir_all(data)?;
            let mut conn = db::open(&catalog, Storage::Local)?;
            let rep = vf_fs::rebuild::rebuild(&mut conn, cabinet, &device_id(data)?)?;
            writeln!(out, "{rep}")?;
            Ok(0)
        }
        Command::Ocr {
            cmd: OcrCmd::Backlog { cabinet, threads },
        } => ocr_backlog(&catalog, cabinet, data, *threads, out),
    }
}

/// `ocr backlog`: queue everything not yet read, run the workers until the queue is empty.
fn ocr_backlog(
    catalog: &Path,
    cabinet: &Path,
    data: &Path,
    threads: Option<usize>,
    out: &mut dyn Write,
) -> Result<i32> {
    use vf_ocr::queue::{self, Counts};
    if !catalog.is_file() {
        bail!("no catalog at {}", catalog.display());
    }
    let config = crate::config::load(data)?;
    let mut conn = db::open(catalog, Storage::Local)?;
    // Queue the backlog here, so the first count already sees it.
    queue::recover(&conn)?;
    queue::enqueue_missing(&conn, queue::BACKLOG)?;
    let q = crate::ui::ocr::start_queue(
        catalog,
        cabinet,
        &config.ocr,
        &config.device_id,
        threads.unwrap_or_else(queue::default_threads),
        || {},
    )
    .map_err(anyhow::Error::msg)?;
    let start = std::time::Instant::now();
    let mut last = None;
    let mut idle_since: Option<std::time::Instant> = None;
    let c = loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let c = Counts::load(&conn)?;
        let status = q.status();
        if last.as_ref() != Some(&c) {
            writeln!(
                out,
                "{} to read, {} reading, {} waiting for the text layer, {} done, {} failed",
                c.queued, c.running, c.indexed, c.done, c.failed
            )?;
            last = Some(c.clone());
            idle_since = None;
        }
        if c.queued + c.running == 0 {
            break c;
        }
        if let Err(why) = &status.engine
            && c.running == 0
            && c.queued > 0
        {
            let t = *idle_since.get_or_insert_with(std::time::Instant::now);
            if t.elapsed() > std::time::Duration::from_secs(10) {
                writeln!(out, "{why}")?;
                break c;
            }
        }
    };
    q.stop_and_wait(std::time::Duration::from_secs(60));
    // Text layers still waiting (the workers write theirs as they go): write them now, then
    // count again. Ones Filer's undoable batch holds stay waiting (exit 3).
    let (written, errors) = vf_ocr::writeback::write_ready(&mut conn, cabinet, &config.device_id);
    for e in errors {
        writeln!(out, "text layer: {e}")?;
    }
    let c = if written > 0 { Counts::load(&conn)? } else { c };
    writeln!(
        out,
        "{} documents searchable, {} text layers written, {} failed ({} s)",
        c.done,
        q.status().written + written,
        c.failed,
        start.elapsed().as_secs()
    )?;
    Ok(if c.failed > 0 {
        4
    } else if c.queued + c.running + c.indexed > 0 {
        3
    } else {
        0
    })
}

/// This workstation's device id, from `<data>/config.toml`.
pub fn device_id(data: &Path) -> Result<String> {
    Ok(crate::config::load(data)?.device_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::audit::{AuditEntry, append};

    #[test]
    fn audit_verify_reports_intact_then_broken() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = db::open(&tmp.path().join("catalog.db"), Storage::Local).unwrap();
        for i in 0..5 {
            let e = AuditEntry::new("file", None, serde_json::json!({ "i": i }));
            append(&conn, "2026-11-02T00:00:00Z", None, "d", &e).unwrap();
        }
        let cmd = Command::Audit {
            cmd: AuditCmd::Verify,
        };
        let mut out = Vec::new();
        assert_eq!(run(&cmd, tmp.path(), &mut out).unwrap(), 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "audit chain intact: 5 rows\n"
        );

        conn.execute_batch(
            "DROP TRIGGER audit_no_update; UPDATE audit SET action = 'x' WHERE seq = 3;",
        )
        .unwrap();
        let mut out = Vec::new();
        assert_eq!(run(&cmd, tmp.path(), &mut out).unwrap(), 2);
        assert!(String::from_utf8(out).unwrap().contains("seq 3"));
    }

    /// `vibe-files ocr backlog` indexes an imported cabinet and exits 0 when everything is
    /// searchable.
    #[test]
    fn ocr_backlog_indexes_an_imported_cabinet() {
        let tmp = tempfile::tempdir().unwrap();
        let cab = tmp.path().join("cab");
        std::fs::create_dir_all(cab.join("Old")).unwrap();
        filer_pdf::pdf::write_test_pdf(
            &cab.join("Old/letter.pdf"),
            &[
                "Engagement letter for Acme LLC 2025",
                "Second page of the letter",
            ],
        );
        vf_fs::cabinet::create(
            &cab,
            &vf_fs::cabinet::CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: None,
            },
        )
        .unwrap();
        let data = tmp.path().join("data");
        let mut out = Vec::new();
        run(
            &Command::Rebuild {
                cabinet: cab.clone(),
            },
            &data,
            &mut out,
        )
        .unwrap();
        let mut out = Vec::new();
        let code = run(
            &Command::Ocr {
                cmd: OcrCmd::Backlog {
                    cabinet: cab,
                    threads: Some(2),
                },
            },
            &data,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("1 documents searchable"), "{text}");
        let conn = db::open(&data.join("catalog.db"), Storage::Local).unwrap();
        let hits = vf_index::search::search(
            &conn,
            &vf_index::search::Query {
                text: "Second page".into(),
                ..Default::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, Some(1));
    }
}
