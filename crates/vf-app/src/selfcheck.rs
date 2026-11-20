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

/// Whether an error's text is the catalog being busy (another connection writing: the index,
/// a sync, another login's update), not something wrong with it.
pub fn is_busy_text(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    e.contains("database is locked")
        || e.contains("database is busy")
        || e.contains("database table is locked")
}

fn check(name: &'static str, r: Result<String, String>) -> Check {
    match r {
        Ok(detail) => Check {
            name,
            ok: true,
            detail,
        },
        // Busy is not a failed check (QA 2026-11-17: a healthy catalog another connection
        // was writing showed "Catalog integrity: database is locked" in red). Not checked
        // this time; it is at the next open.
        Err(e) if is_busy_text(&e) => Check {
            name,
            ok: true,
            detail: "not checked just now (the catalog was busy)".into(),
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
    let one_main = crate::server::server_mode() || crate::cloudmode::is_cloud(root);
    // The computer B2 sync runs on, when it is another one, and what the cabinet says of its
    // last whole push (each read once). `one_main`: the cabinet syncs on one main computer
    // (several servers, a cloud cabinet); on a plain office share every PC syncs, and a
    // main-server file left from before means nothing.
    let (main, last_push) = match main_elsewhere(root, one_main) {
        Some((m, p)) => (Some(m), p),
        None => (None, None),
    };
    if sync_configured {
        // The cabinet's notes of which computer syncs, or of its last push, could not be
        // read just now: not checked, rather than "has not sent the cabinet to B2 yet" or
        // "this PC has not pushed" in red (QA 2026-11-17).
        let unread =
            one_main && (crate::server::main_unreadable(root) || crate::lastpush::unreadable(root));
        out.push(check(
            "Sync",
            match &main {
                _ if unread => Ok(
                    "not checked just now (the cabinet's sync notes could not be read)".into(),
                ),
                // Named, and it has never sent the cabinet: nothing is going to B2, whatever
                // this catalog's own state.
                Some(m) if last_push.is_none() && m.computer.is_empty() => Err(
                    "the whole cabinet has not been sent to B2 yet: it goes from the first server with Vibe Files open and sync unlocked".into(),
                ),
                Some(m) if last_push.is_none() => Err(format!(
                    "set to run on {}, which has not sent the cabinet to B2 yet (Admin › Devices › Servers)",
                    m.computer
                )),
                // On a computer B2 sync does not run on, a catalog that never pushed or
                // pulled is as it should be (owner 2026-11-16: a session host said so in red
                // every morning). Only on the word of a push that did happen: "B2 backup"
                // below says how old it is.
                Some(m) => sync_state(conn, Some(&m.computer)),
                None => sync_state(conn, None),
            },
        ));
    }
    // On the computers that are not the main one (sync set up there or not): the main
    // computer, or its scheduled push, stopped sending the cabinet to B2 (PA-40). The main one
    // itself is not told: it opens before its first push of the day (a Monday after a weekend
    // closed); its own Sync check covers it.
    out.extend(last_push.map(|r| check("B2 backup", r)));
    out
}

/// The other computer the cabinet's B2 sync runs on, and what the cabinet says of its last
/// whole push (`None`: it never sent one). `None` when sync runs here, or on every PC.
fn main_elsewhere(
    root: &Path,
    one_main: bool,
) -> Option<(crate::server::MainServer, Option<Result<String, String>>)> {
    // Several servers (PA-99): the whole cabinet goes from whichever server holds the sync
    // lease, so every server but the holder is told how old the last whole push is.
    if one_main && crate::lease::applies(root) {
        let who = crate::lease::holder_computer(root).unwrap_or_default();
        if who.eq_ignore_ascii_case(&crate::server::computer()) {
            return None;
        }
        let main = crate::server::MainServer {
            computer: who,
            since: String::new(),
        };
        return Some((main, crate::lastpush::check(root)));
    }
    let main = one_main
        .then(|| crate::server::main_server(root))
        .flatten()
        .filter(|m| !m.computer.eq_ignore_ascii_case(&crate::server::computer()))?;
    Some((main, crate::lastpush::check(root)))
}

/// "B2 backup", when another computer runs the cabinet's B2 sync and it sent the whole cabinet
/// once at least.
#[cfg(test)]
fn backup_check(root: &Path, one_main: bool) -> Option<Check> {
    main_elsewhere(root, one_main)?
        .1
        .map(|r| check("B2 backup", r))
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

/// This catalog's own sync state. `runs_on`: the other computer B2 sync runs on (and has
/// pushed from): never having exchanged, or not lately, is then no failure here.
fn sync_state(conn: &Connection, runs_on: Option<&str>) -> Result<String, String> {
    let conflicts: i64 = conn
        .query_row(
            "SELECT count(*) FROM sync_conflict WHERE resolved IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    // A read that failed is an error (busy: not checked this time), not "never exchanged".
    let last: Option<String> = conn
        .query_row(
            "SELECT max(coalesce(last_pull, ''), coalesce(last_push, '')) FROM sync_state",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .or_else(|e| match e {
            vf_core::db::SqlError::QueryReturnedNoRows => Ok(None),
            e => Err(e.to_string()),
        })?
        .filter(|s| !s.is_empty());
    if conflicts > 0 {
        return Err(format!("{conflicts} conflicts to resolve (Sync tab)"));
    }
    let Some(last) = last else {
        if let Some(c) = runs_on {
            return Ok(if c.is_empty() {
                "runs on the server holding the sync lease".into()
            } else {
                format!("runs on {c}")
            });
        }
        // (Not "unlock it": this is said when sync is unlocked too, owner 2026-11-16.)
        return Err(
            "set up, but this PC's catalog has not pushed to B2 or pulled from it yet: Push now or Pull now in the Sync tab"
                .into(),
        );
    };
    let age = chrono::DateTime::parse_from_rfc3339(&last)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days())
        .unwrap_or(0);
    if age > 7 && runs_on.is_none() {
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
        // On a computer B2 sync does not run on, that is as it should be.
        assert!(sync_state(&conn, Some("HOST-2")).is_ok());
        assert!(sync_state(&conn, None).is_err());
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
