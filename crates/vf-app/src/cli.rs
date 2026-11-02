//! Command-line subcommands (`vibe-files <cmd>`). With no subcommand the app opens its window.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use vf_core::db::{self, CatalogKey, Storage};

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
}

#[derive(Subcommand, Debug)]
pub enum AuditCmd {
    /// Verify the audit hash chain. Exit code 0 = intact, 2 = broken.
    Verify,
}

/// How the CLI gets the raw catalog key until P06 adds DPAPI and the recovery passphrase:
/// `--key-file` (64 hex chars) or the `VF_CATALOG_KEY` environment variable.
pub fn load_key(key_file: Option<&Path>) -> Result<CatalogKey> {
    let text = match key_file {
        Some(p) => {
            std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
        }
        None => std::env::var("VF_CATALOG_KEY")
            .context("no catalog key: pass --key-file or set VF_CATALOG_KEY (64 hex chars)")?,
    };
    parse_key(text.trim())
}

pub fn parse_key(hex: &str) -> Result<CatalogKey> {
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the catalog key must be 64 hex characters");
    }
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("checked hex");
    }
    Ok(CatalogKey(k))
}

/// Runs one subcommand; returns the process exit code.
pub fn run(cmd: &Command, data: &Path, key: &CatalogKey, out: &mut dyn Write) -> Result<i32> {
    let catalog = data.join("catalog.db");
    match cmd {
        Command::Audit {
            cmd: AuditCmd::Verify,
        } => {
            if !catalog.is_file() {
                bail!("no catalog at {}", catalog.display());
            }
            let conn = db::open(&catalog, key, Storage::Local)?;
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
            let mut conn = db::open(&catalog, key, Storage::Local)?;
            let rep = vf_fs::rebuild::rebuild(&mut conn, cabinet, &device_id(data)?)?;
            writeln!(out, "{rep}")?;
            Ok(0)
        }
    }
}

/// This workstation's device id (uuid v7), kept in `<data>/device_id` until `config.toml` lands
/// with P01.
pub fn device_id(data: &Path) -> Result<String> {
    let p = data.join("device_id");
    if let Ok(s) = std::fs::read_to_string(&p) {
        return Ok(s.trim().to_owned());
    }
    let id = vf_core::new_id();
    std::fs::write(&p, &id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::audit::{AuditEntry, append};

    #[test]
    fn audit_verify_reports_intact_then_broken() {
        let tmp = tempfile::tempdir().unwrap();
        let key = parse_key(&"ab".repeat(32)).unwrap();
        let conn = db::open(&tmp.path().join("catalog.db"), &key, Storage::Local).unwrap();
        for i in 0..5 {
            let e = AuditEntry::new("file", None, serde_json::json!({ "i": i }));
            append(&conn, "2026-11-02T00:00:00Z", None, "d", &e).unwrap();
        }
        let cmd = Command::Audit {
            cmd: AuditCmd::Verify,
        };
        let mut out = Vec::new();
        assert_eq!(run(&cmd, tmp.path(), &key, &mut out).unwrap(), 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "audit chain intact: 5 rows\n"
        );

        conn.execute_batch(
            "DROP TRIGGER audit_no_update; UPDATE audit SET action = 'x' WHERE seq = 3;",
        )
        .unwrap();
        let mut out = Vec::new();
        assert_eq!(run(&cmd, tmp.path(), &key, &mut out).unwrap(), 2);
        assert!(String::from_utf8(out).unwrap().contains("seq 3"));
    }

    #[test]
    fn key_must_be_64_hex() {
        assert!(parse_key("abc").is_err());
        assert!(parse_key(&"zz".repeat(32)).is_err());
        assert!(parse_key(&"0F".repeat(32)).is_ok());
    }
}
