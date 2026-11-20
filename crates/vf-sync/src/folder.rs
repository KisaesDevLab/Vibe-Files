//! What the shared cabinet folder holds of the bucket (PA-99).
//!
//! On several servers each one keeps its own catalog of one shared folder, and the whole
//! cabinet syncs on whichever server holds the sync lease. A catalog's own record of the last
//! exchange (`sync_segment`, `sync_base`) only says what *that* catalog exchanged; the folder
//! may have taken in later segments through another server. `_Vibe/sync-folder.json` says, for
//! each segment (a client's, or the cabinet's under `""`), the latest rev the folder holds and
//! the object it was read from. A segment at that rev is already in the folder: it is taken as
//! the last exchange without writing a file. A newer one from a computer with its own copy is
//! applied three ways against the folder's rev, not against a catalog's older one (which would
//! take every change made here since for a change in both places, and leave conflict copies).
//!
//! The record is a hint, written after the exchange: one that cannot be read or written leaves
//! the three-way apply as it was before PA-99. It is never synced (not a master file).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "sync-folder.json";

/// One segment as the folder holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub rev: u64,
    /// The segment's object in the bucket.
    pub key: String,
}

fn path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(FILE)
}

/// The record; empty when there is none or it cannot be read.
pub fn load(root: &Path) -> BTreeMap<String, Held> {
    std::fs::read(path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Notes that the folder now holds these segments `(segment, rev, key)`. A rev older than the
/// one recorded is not written back over it.
pub fn record(root: &Path, held: &[(String, u64, String)]) {
    if held.is_empty() {
        return;
    }
    let r = vf_fs::lock::with(root, "sync-folder", "the sync record", || {
        let mut all = load(root);
        let mut changed = false;
        for (seg, rev, key) in held {
            if all.get(seg).is_none_or(|h| h.rev < *rev) {
                all.insert(
                    seg.clone(),
                    Held {
                        rev: *rev,
                        key: key.clone(),
                    },
                );
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        let body = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        vf_fs::cabinet::write_replace(&path(root), &body)
    });
    if let Err(e) = r {
        // A hint only: the next exchange falls back to the three-way apply.
        tracing::warn!(error = %e, "sync: the shared folder's record was not written");
    }
}
