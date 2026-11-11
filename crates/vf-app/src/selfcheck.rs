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

/// Runs every check when a cabinet opens. The catalog integrity check reads the whole file
/// (a minute or more for ~900K documents, PA-34): once a month (Q-68).
pub fn run(conn: &Connection, root: &Path, sync_configured: bool) -> Vec<Check> {
    run_with(conn, root, sync_configured, false)
}

/// Runs every check. `sync_configured`: sync is set up for this cabinet on this PC.
/// `catalog_always`: the catalog integrity check runs even when this month's has run
/// (`vibe-files check`).
pub fn run_with(
    conn: &Connection,
    root: &Path,
    sync_configured: bool,
    catalog_always: bool,
) -> Vec<Check> {
    let mut out = vec![];
    out.push(check(
        "Catalog integrity",
        if catalog_always || vf_core::db::health_check_due(conn) {
            vf_core::db::health_check(conn)
                .map_err(|e| e.to_string())
                .and_then(|problems| {
                    if problems.is_empty() {
                        Ok("ok".into())
                    } else {
                        Err(format!(
                            "the catalog is damaged ({}); run `vibe-files rebuild`",
                            problems.join("; ")
                        ))
                    }
                })
        } else {
            Ok("checked this month".into())
        },
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
        // A change running right now has a row too: only rows over 10 minutes old count.
        // Another session's change in progress on a shared catalog does not count.
        vf_fs::fileops::interrupted(conn, 600).and_then(|n| {
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
    // On the computers that are not the main one (sync set up there or not): the main
    // computer, or its scheduled push, stopped sending the cabinet to B2 (PA-40). The main one
    // itself is not told: it opens before its first push of the day (a Monday after a weekend
    // closed); its own Sync check covers it.
    let one_main = crate::server::server_mode() || crate::cloudmode::is_cloud(root);
    out.extend(backup_check(root, one_main));
    out
}

/// "B2 backup", when another computer runs the cabinet's B2 sync and it sent the whole cabinet
/// once at least. `one_main`: the cabinet syncs on one main computer (several servers, a cloud
/// cabinet); on a plain office share every PC syncs, and a main-server file left from before
/// means nothing.
fn backup_check(root: &Path, one_main: bool) -> Option<Check> {
    let another = one_main
        && crate::server::main_server(root)
            .is_some_and(|m| !m.computer.eq_ignore_ascii_case(&crate::server::computer()));
    if !another {
        return None;
    }
    crate::lastpush::check(root).map(|r| check("B2 backup", r))
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

    /// PA-40: B2 falling behind is shown on the computers that are not the main one, not on
    /// the main one (it opens before its first push of the day).
    #[test]
    fn a_stale_backup_is_reported_away_from_the_main_computer() {
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
        let backup = |root: &Path| backup_check(root, true);
        std::fs::write(
            root.join("_Vibe/sync-last-push.json"),
            r#"{"computer":"SERVER-1","at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        // This computer is the main one: not told.
        crate::server::set_main(&root).unwrap();
        assert!(backup(&root).is_none());
        // Another computer is.
        std::fs::write(
            root.join("_Vibe/main-server.json"),
            r#"{"computer":"SERVER-1","since":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let c = backup(&root).unwrap();
        assert!(!c.ok && c.detail.contains("SERVER-1"), "{c:?}");
        // A plain office share (every PC syncs): a main-server file left from before says nothing.
        assert!(backup_check(&root, false).is_none());
        // Never pushed in full: nothing to say.
        std::fs::remove_file(root.join("_Vibe/sync-last-push.json")).unwrap();
        assert!(backup(&root).is_none());
    }
}
