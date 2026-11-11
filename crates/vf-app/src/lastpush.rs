//! The cabinet's last full B2 push (PA-40), kept in `_Vibe/sync-last-push.json` so every
//! computer sees it: a server's own catalog knows only its own pushes, and B2 sync runs on the
//! main server alone. Written after each whole-cabinet push that finished, from the app or
//! `vibe-files sync <cabinet> push`, even when there was nothing to send. Not a master file:
//! never sent to the bucket.

use std::path::{Path, PathBuf};

/// Longer than this since the last full push: the startup check says so.
pub const STALE_HOURS: i64 = 24;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LastPush {
    /// The computer that pushed.
    pub computer: String,
    /// When it finished (RFC 3339, UTC).
    pub at: String,
}

fn path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR)
        .join("sync-last-push.json")
}

/// Records a full push that finished now on this computer. A failure is logged only: the push
/// itself went up.
pub fn record(root: &Path) {
    let p = LastPush {
        computer: crate::server::computer(),
        at: vf_core::now_rfc3339(),
    };
    let r = serde_json::to_vec_pretty(&p)
        .map_err(|e| e.to_string())
        .and_then(|b| vf_fs::cabinet::write_replace(&path(root), &b));
    if let Err(e) = r {
        tracing::warn!(kind = crate::logging::kind(&e), "last push not recorded");
    }
}

pub fn read(root: &Path) -> Option<LastPush> {
    serde_json::from_slice(&std::fs::read(path(root)).ok()?).ok()
}

/// The startup check: `None` when this cabinet has never been pushed in full (sync not used),
/// else whether the last full push is recent enough.
pub fn check(root: &Path) -> Option<Result<String, String>> {
    let last = read(root)?;
    let hours = chrono::DateTime::parse_from_rfc3339(&last.at)
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_hours())
        .unwrap_or(i64::MAX);
    let when = crate::ui::widgets::local_time(&last.at);
    Some(if hours > STALE_HOURS {
        Err(format!(
            "last sent to B2 {when} by {}: is Vibe Files or its scheduled push running there?",
            last.computer
        ))
    } else {
        Ok(format!("last sent {when} by {}", last.computer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_over_a_day_old_is_reported() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join(vf_fs::cabinet::VIBE_DIR)).unwrap();
        assert!(check(root).is_none(), "never pushed: no check");
        record(root);
        let ok = check(root).unwrap();
        assert!(ok.is_ok(), "{ok:?}");
        assert_eq!(read(root).unwrap().computer, crate::server::computer());
        let old = LastPush {
            computer: "SERVER-1".into(),
            at: "2026-01-01T00:00:00Z".into(),
        };
        std::fs::write(path(root), serde_json::to_vec(&old).unwrap()).unwrap();
        let err = check(root).unwrap().unwrap_err();
        assert!(err.contains("SERVER-1"), "{err}");
        // Never sent up with the cabinet's master files.
        assert!(!vf_sync::push::master_files(root).contains_key("sync-last-push.json"));
    }
}
