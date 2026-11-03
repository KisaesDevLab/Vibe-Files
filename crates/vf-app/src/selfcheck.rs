//! Startup self-checks (PHASES P13 step 4): catalog integrity, audit chain, cabinet reachable
//! and writable, interrupted changes put back, sync state. They run on a thread right after a
//! cabinet opens (the catalog check reads the whole file); anything not OK shows on Browse.
//! `vibe-files check <cabinet>` runs the same list.

use std::path::Path;

use vf_core::db::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

fn check(name: &'static str, r: Result<String, String>) -> Check {
    match r {
        Ok(detail) => Check {
            name,
            ok: true,
            detail,
        },
        Err(detail) => Check {
            name,
            ok: false,
            detail,
        },
    }
}

/// Runs every check. `sync_configured`: sync is set up for this cabinet on this PC.
pub fn run(conn: &Connection, root: &Path, sync_configured: bool) -> Vec<Check> {
    let mut out = vec![];
    out.push(check(
        "Catalog integrity",
        conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())
            .and_then(|s| {
                if s == "ok" {
                    Ok("ok".into())
                } else {
                    Err(format!(
                        "the catalog is damaged ({s}); run `vibe-files rebuild`"
                    ))
                }
            }),
    ));
    out.push(check(
        "Audit chain",
        vf_core::audit::verify(conn)
            .map_err(|e| e.to_string())
            .and_then(|r| match r.broken {
                None => Ok(format!("{} rows intact", r.rows)),
                Some((seq, why)) => Err(format!("broken at row {seq}: {why}")),
            }),
    ));
    out.push(check("Cabinet reachable", reachable(root)));
    out.push(check(
        "Interrupted changes",
        conn.query_row("SELECT count(*) FROM tx_journal", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|e| e.to_string())
        .and_then(|n| {
            if n == 0 {
                Ok("none left".into())
            } else {
                Err(format!(
                    "{n} changes cut short were not put back: reopen the cabinet"
                ))
            }
        }),
    ));
    if sync_configured {
        out.push(check("Sync", sync_state(conn)));
    }
    out
}

/// The cabinet file reads and the cabinet takes a write (a share gone read-only fails here).
fn reachable(root: &Path) -> Result<String, String> {
    vf_fs::cabinet::read(root)?;
    let dir = vf_fs::cabinet::tmp_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot write in the cabinet: {e}"))?;
    let probe = dir.join(format!("probe-{}", vf_core::new_id()));
    std::fs::write(&probe, b"ok").map_err(|e| format!("cannot write in the cabinet: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok("readable and writable".into())
}

fn sync_state(conn: &Connection) -> Result<String, String> {
    let conflicts: i64 = conn
        .query_row(
            "SELECT count(*) FROM sync_conflict WHERE resolved IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let last: Option<String> = conn
        .query_row(
            "SELECT max(coalesce(last_pull, ''), coalesce(last_push, '')) FROM sync_state",
            [],
            |r| r.get(0),
        )
        .ok()
        .filter(|s: &String| !s.is_empty());
    if conflicts > 0 {
        return Err(format!("{conflicts} conflicts to resolve (Sync tab)"));
    }
    let Some(last) = last else {
        return Err("set up but never run: unlock it in the Sync tab".into());
    };
    let age = chrono::DateTime::parse_from_rfc3339(&last)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days())
        .unwrap_or(0);
    if age > 7 {
        Err(format!("last exchange {age} days ago"))
    } else {
        Ok(format!(
            "last exchange {}",
            crate::ui::widgets::local_time(&last)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::db::Storage;

    #[test]
    fn checks_pass_on_a_fresh_cabinet_and_fail_when_it_is_gone() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        vf_fs::cabinet::create(
            &root,
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
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        let r = run(&conn, &root, true);
        assert!(r.iter().take(4).all(|c| c.ok), "{r:?}");
        let sync = r.iter().find(|c| c.name == "Sync").unwrap();
        assert!(!sync.ok, "set up but never run");
        std::fs::remove_dir_all(&root).unwrap();
        let r = run(&conn, &root, false);
        assert!(!r.iter().find(|c| c.name == "Cabinet reachable").unwrap().ok);
    }
}
