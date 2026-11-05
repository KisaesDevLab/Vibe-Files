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
    /// The startup self-checks: catalog integrity, audit chain, cabinet reachable, interrupted
    /// changes, sync. Exit code 0 = all OK, 5 = something failed.
    Check {
        /// Cabinet root folder.
        cabinet: PathBuf,
    },
    /// B2 sync of the cabinet (set it up in the app first). The sync passphrase is read from
    /// `VF_SYNC_PASSPHRASE`, else from one line on stdin.
    Sync {
        /// Cabinet root folder.
        cabinet: PathBuf,
        #[command(subcommand)]
        cmd: SyncCmd,
    },
    /// One client's documents, its audit trail extract (CSV) and a manifest (paths, SHA-256,
    /// doc types, years) in one zip.
    ExportClient {
        /// Cabinet root folder.
        cabinet: PathBuf,
        /// The client's ID (tax software / CSV ID).
        id: String,
        /// The zip to write.
        out: PathBuf,
    },
    /// Restore a cabinet from its B2 bucket onto this PC (disaster recovery). The application
    /// key comes from `VF_S3_APP_KEY` and the sync passphrase from `VF_SYNC_PASSPHRASE` (else one
    /// line each on stdin). Without `--cabinet-id`, lists the cabinets in the bucket.
    Restore {
        /// New, empty folder for the cabinet.
        folder: PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        region: String,
        #[arg(long)]
        bucket: String,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        cabinet_id: Option<String>,
    },
    /// Memory and handle soak test on a synthetic cabinet in a new folder (P13 step 3). Exit
    /// code 0 = steady, 6 = kept growing.
    Soak {
        /// How long to run.
        #[arg(long, default_value_t = 8.0)]
        hours: f64,
        /// Working folder (new); default: `<data>/soak-<time>`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SyncCmd {
    /// Pull what other workstations sent, then send what changed here.
    Push,
    /// Bring in what other workstations sent.
    Pull,
    /// Pending documents, conflicts, last exchange.
    Status,
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

impl Command {
    /// Whether the command changes the catalog or the cabinet (then it needs the data folder
    /// to itself: not while the app has it open).
    pub fn changes_things(&self) -> bool {
        !matches!(
            self,
            Command::Audit { .. }
                | Command::Check { .. }
                | Command::Sync {
                    cmd: SyncCmd::Status,
                    ..
                }
                | Command::ExportClient { .. }
                | Command::Soak { .. }
        )
    }
}

/// Refuses a cabinet the catalog does not belong to (a typo, a second cabinet): a rebuild there
/// would tombstone every document of the catalog's own cabinet (QA A3).
fn same_cabinet(conn: &db::Connection, cabinet: &Path) -> Result<()> {
    let cab = vf_fs::cabinet::read(cabinet).map_err(anyhow::Error::msg)?;
    let (n, known): (i64, i64) = conn.query_row(
        "SELECT count(*), coalesce(sum(id = ?1), 0) FROM cabinet",
        [&cab.cabinet_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if n > 0 && known == 0 {
        bail!(
            "this data folder's catalog belongs to a different cabinet than {}: use the data folder of that cabinet (--data)",
            cabinet.display()
        );
    }
    Ok(())
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
            same_cabinet(&conn, cabinet)?;
            let rep = vf_fs::rebuild::rebuild(&mut conn, cabinet, &device_id(data)?)?;
            writeln!(out, "{rep}")?;
            Ok(0)
        }
        Command::Ocr {
            cmd: OcrCmd::Backlog { cabinet, threads },
        } => ocr_backlog(&catalog, cabinet, data, *threads, out),
        Command::Check { cabinet } => {
            if !catalog.is_file() {
                bail!("no catalog at {}", catalog.display());
            }
            let conn = db::open(&catalog, Storage::Local)?;
            same_cabinet(&conn, cabinet)?;
            // Read-only: interrupted changes are put back when the app opens the cabinet (a check
            // run beside a working app must not move its files).
            let config = crate::config::load(data)?;
            let cab = vf_fs::cabinet::read(cabinet).map_err(anyhow::Error::msg);
            let synced = cab
                .as_ref()
                .is_ok_and(|c| config.sync.configured_for(&c.cabinet_id));
            let checks = crate::selfcheck::run_with(&conn, cabinet, synced, true);
            for c in &checks {
                writeln!(
                    out,
                    "{} {}: {}",
                    if c.ok { "ok  " } else { "FAIL" },
                    c.name,
                    c.detail
                )?;
            }
            Ok(if checks.iter().all(|c| c.ok) { 0 } else { 5 })
        }
        Command::Sync { cabinet, cmd } => sync(&catalog, cabinet, data, cmd, out),
        Command::ExportClient {
            cabinet,
            id,
            out: zip,
        } => {
            let r = export_client(&catalog, cabinet, id, zip, out);
            if r.is_err() && zip.exists() {
                // Nothing half-written stays behind (the next try would find it "exists").
                let _ = std::fs::remove_file(zip);
            }
            r
        }
        Command::Restore {
            folder,
            endpoint,
            region,
            bucket,
            key_id,
            cabinet_id,
        } => {
            let secret = |var: &str| -> Result<String> {
                Ok(match std::env::var(var) {
                    Ok(v) => v,
                    Err(_) => read_secret(var)?,
                })
            };
            let store = vf_sync::Remote::s3(
                &vf_sync::remote::S3Config {
                    endpoint: endpoint.clone(),
                    region: region.clone(),
                    bucket: bucket.clone(),
                    key_id: key_id.clone(),
                },
                &secret("VF_S3_APP_KEY")?,
            )?;
            let Some(id) = cabinet_id else {
                for c in vf_sync::restore::cabinets(store)? {
                    writeln!(out, "{c}")?;
                }
                return Ok(0);
            };
            if catalog.exists() {
                bail!(
                    "{} already holds a catalog: use a new --data folder",
                    data.display()
                );
            }
            std::fs::create_dir_all(data)?;
            let mut conn = db::open(&catalog, Storage::Local)?;
            let device = vf_core::new_id();
            let r = vf_sync::restore::restore(
                &mut conn,
                folder,
                store,
                id,
                &secret("VF_SYNC_PASSPHRASE")?,
                &device,
                &std::env::var("COMPUTERNAME").unwrap_or_else(|_| "restore".into()),
            )?;
            writeln!(
                out,
                "restored {} clients, {} documents, {} months of audit trail into {}; open it with Cabinet › Open existing",
                r.clients,
                r.documents,
                r.audit_months,
                folder.display()
            )?;
            Ok(0)
        }
        Command::Soak { hours, dir } => {
            let dir = dir.clone().unwrap_or_else(|| {
                data.join(format!(
                    "soak-{}",
                    chrono::Local::now().format("%Y%m%d-%H%M%S")
                ))
            });
            crate::soak::run(&dir, *hours, out)
        }
    }
}

/// One line from stdin for the secret `var` names. Typed at a console it is not shown (QA X8);
/// piped in, it is read as it comes.
fn read_secret(var: &str) -> std::io::Result<String> {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };
    // SAFETY: plain console calls on this process's stdin handle.
    let console = unsafe {
        GetStdHandle(STD_INPUT_HANDLE).ok().and_then(|h| {
            let mut mode = CONSOLE_MODE::default();
            GetConsoleMode(h, &mut mode).ok().map(|_| (h, mode))
        })
    };
    if let Some((h, mode)) = console {
        eprint!("{var} (not shown): ");
        // SAFETY: as above; the mode is put back below.
        let _ = unsafe { SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT) };
    }
    let mut l = String::new();
    let r = std::io::stdin().read_line(&mut l);
    if let Some((h, mode)) = console {
        // SAFETY: restores the mode read above.
        let _ = unsafe { SetConsoleMode(h, mode) };
        eprintln!();
    }
    r?;
    Ok(l.trim_end_matches(['\r', '\n']).to_string())
}

/// An unlocked sync session from `config.toml [sync]` and the passphrase.
fn sync_session(cabinet: &Path, data: &Path) -> Result<vf_sync::Sync> {
    let config = crate::config::load(data)?;
    let cab = vf_fs::cabinet::read(cabinet).map_err(anyhow::Error::msg)?;
    if !config.sync.configured_for(&cab.cabinet_id) {
        bail!("sync is not set up for this cabinet on this PC (Sync tab)");
    }
    let connect = |keys| {
        vf_sync::Sync::connect(
            keys,
            &config.sync.s3(),
            &config.sync.app_key_sealed,
            &cab.cabinet_id,
            &config.device_id,
        )
    };
    // The key this login remembered on this PC (Q-38); a stale one (sync set up again since)
    // falls back to the passphrase.
    if let Some(keys) = vf_sync::setup::recall(data, &cab.cabinet_id)
        && let Ok(s) = connect(keys)
    {
        return Ok(s);
    }
    let pass = match std::env::var("VF_SYNC_PASSPHRASE") {
        Ok(p) => p,
        Err(_) => read_secret("VF_SYNC_PASSPHRASE")?,
    };
    let keys = vf_sync::setup::unlock_local(cabinet, &cab.cabinet_id, &pass)?;
    Ok(connect(keys)?)
}

fn sync(
    catalog: &Path,
    cabinet: &Path,
    data: &Path,
    cmd: &SyncCmd,
    out: &mut dyn Write,
) -> Result<i32> {
    if !catalog.is_file() {
        bail!("no catalog at {}", catalog.display());
    }
    let mut conn = db::open(catalog, Storage::Local)?;
    same_cabinet(&conn, cabinet)?;
    match cmd {
        SyncCmd::Status => {
            let pending = vf_sync::catalog::pending(&conn)?;
            let conflicts = vf_sync::restore::conflicts(&conn)?;
            let (rev, push, pull): (i64, Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT last_manifest_rev, last_push, last_pull FROM sync_state LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap_or((0, None, None));
            writeln!(
                out,
                "{} documents pending in {} segments · {} conflicts · manifest rev {rev} · last push {} · last pull {}",
                pending.values().sum::<usize>(),
                pending.len(),
                conflicts.len(),
                push.as_deref().unwrap_or("never"),
                pull.as_deref().unwrap_or("never"),
            )?;
            Ok(0)
        }
        SyncCmd::Push => {
            let s = sync_session(cabinet, data)?;
            let r = vf_sync::push::push(&mut conn, cabinet, &s, None)?;
            writeln!(
                out,
                "pushed {} documents ({} objects, {} bytes) · rev {}",
                r.documents, r.objects, r.bytes, r.head_rev
            )?;
            Ok(0)
        }
        SyncCmd::Pull => {
            let s = sync_session(cabinet, data)?;
            let r = vf_sync::pull::pull(&mut conn, cabinet, &s)?;
            writeln!(
                out,
                "pulled: {} new, {} updated, {} moved, {} re-typed, {} deleted, {} conflicts · rev {}",
                r.new, r.updated, r.moved, r.meta, r.deleted, r.conflicts, r.head_rev
            )?;
            Ok(0)
        }
    }
}

/// `export-client`: `files/<cabinet path>`, `audit.csv`, `manifest.json`.
fn export_client(
    catalog: &Path,
    cabinet: &Path,
    ext_id: &str,
    zip_path: &Path,
    out: &mut dyn Write,
) -> Result<i32> {
    use std::io::Write as _;
    if !catalog.is_file() {
        bail!("no catalog at {}", catalog.display());
    }
    let conn = db::open(catalog, Storage::Local)?;
    same_cabinet(&conn, cabinet)?;
    let (client_id, name): (String, String) = conn
        .query_row(
            "SELECT id, name FROM client WHERE ext_id = ?1 AND deleted_at IS NULL",
            [ext_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| anyhow::anyhow!("no client with ID {ext_id}"))?;
    let mut st = conn.prepare(
        "SELECT d.rel_path, d.hash, d.size, t.name, d.tax_year, d.status FROM document d
         LEFT JOIN doc_type t ON t.id = d.doc_type_id
         WHERE d.client_id = ?1 AND d.deleted_at IS NULL ORDER BY d.rel_path",
    )?;
    type Row = (String, String, i64, Option<String>, Option<i64>, String);
    let docs: Vec<Row> = st
        .query_map([&client_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    if zip_path.exists() {
        bail!("{} exists already", zip_path.display());
    }
    let f = std::fs::File::create(zip_path)?;
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut manifest = vec![];
    for (rel, hash, size, ty, year, status) in &docs {
        let bytes = std::fs::read(cabinet.join(rel))?;
        let have = vf_sync::crypto::sha256(&bytes);
        if &have != hash {
            bail!(
                "{rel} does not match the catalog (changed outside Vibe Files?): run the integrity sweep first"
            );
        }
        z.start_file(format!("files/{rel}"), opts)?;
        z.write_all(&bytes)?;
        manifest.push(serde_json::json!({
            "path": rel, "sha256": hash, "size": size, "doc_type": ty, "tax_year": year,
            "status": status,
        }));
    }
    let audit = vf_core::audit::query(
        &conn,
        &vf_core::audit::Filter {
            client_id: Some(client_id.clone()),
            ..Default::default()
        },
        None,
    )?;
    z.start_file("audit.csv", opts)?;
    z.write_all(vf_core::audit::to_csv(&audit).as_bytes())?;
    z.start_file("manifest.json", opts)?;
    z.write_all(
        serde_json::to_string_pretty(&serde_json::json!({
            "client": {"id": ext_id, "uuid": client_id, "name": name},
            "exported_at": vf_core::now_rfc3339(),
            "documents": manifest,
            "audit_rows": audit.len(),
        }))?
        .as_bytes(),
    )?;
    z.finish()?;
    writeln!(
        out,
        "{} documents and {} audit rows written to {}",
        docs.len(),
        audit.len(),
        zip_path.display()
    )?;
    Ok(0)
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
    same_cabinet(&conn, cabinet)?;
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

    /// P13 step 5: `check` passes on a healthy cabinet; `export-client` writes the client's
    /// files, its audit extract and a manifest.
    #[test]
    fn check_and_export_client() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        let data = t.path().join("data");
        let req = crate::session::CreateRequest {
            root: &root,
            name: "Firm",
        };
        {
            let mut o = crate::session::create(&data, "dev", &req).unwrap();
            let p = vf_fs::newclient::plan(
                &o.conn,
                &root,
                &vf_fs::newclient::NewClient {
                    ext_id: "1042".into(),
                    name: "Anderson Farms LLC".into(),
                    entity_type: None,
                },
            )
            .unwrap();
            vf_fs::newclient::create(&mut o.conn, &root, &p, "dev").unwrap();
            let f = root.join("Anderson Farms LLC (1042)/Income Tax/2025/w2.pdf");
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, "invented").unwrap();
            vf_fs::rebuild::rebuild(&mut o.conn, &root, "dev").unwrap();
        }
        crate::config::save(
            &data,
            &crate::config::Config {
                device_id: "dev".into(),
                last_cabinet: None,
                prefs: Default::default(),
                ingest: Default::default(),
                ocr: Default::default(),
                viewer: Default::default(),
                look: Default::default(),
                sync: Default::default(),
                update_check: false,
            },
        )
        .unwrap();
        let mut out = Vec::new();
        let code = run(
            &Command::Check {
                cabinet: root.clone(),
            },
            &data,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("ok   Audit chain"), "{text}");

        let zip_path = t.path().join("anderson.zip");
        let mut out = Vec::new();
        let code = run(
            &Command::ExportClient {
                cabinet: root.clone(),
                id: "1042".into(),
                out: zip_path.clone(),
            },
            &data,
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        let mut z = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert!(
            names.contains(&"files/Anderson Farms LLC (1042)/Income Tax/2025/w2.pdf".to_string())
        );
        assert!(names.contains(&"audit.csv".to_string()));
        let mut m = String::new();
        std::io::Read::read_to_string(&mut z.by_name("manifest.json").unwrap(), &mut m).unwrap();
        assert!(m.contains("\"sha256\""), "{m}");
    }

    /// P13 step 3: the soak loop runs (a few seconds here; 8 hours with `vibe-files soak`).
    #[test]
    fn soak_runs_briefly() {
        let t = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        let code = crate::soak::run(&t.path().join("soak"), 0.001, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("iterations"), "{text}");
        assert!(code == 0 || code == 6);
        assert!(t.path().join("soak/soak.csv").is_file());
    }
}
