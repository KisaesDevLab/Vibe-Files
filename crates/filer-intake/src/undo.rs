//! Undo of the most recent batch: files go back to their inboxes (hand-added files to the first
//! inbox, X76), overwritten files are restored, sentinels the batch wrote are removed, and folders
//! the batch created are removed if empty (deepest first).

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::commit::{self, LastBatch};
use crate::oplog::{self, Op, OpRecord, OpResult};

/// What Undo would do, for the confirmation dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoSummary {
    pub files: usize,
    pub restored: usize,
    pub folders: usize,
    /// Copies to related clients (P07a), removed unless changed since.
    pub copies: usize,
}

pub fn summary(lb: &LastBatch) -> UndoSummary {
    UndoSummary {
        files: lb.items.len(),
        restored: lb.backups.len(),
        folders: lb.created_dirs.len(),
        copies: lb.items.iter().map(|i| i.copies.len()).sum::<usize>() + lb.zip_entries.len(),
    }
}

/// Copies of the last batch that were changed since they were written (they stay on Undo).
pub fn modified_copies(lb: &LastBatch) -> Vec<PathBuf> {
    lb.items
        .iter()
        .flat_map(|i| &i.copies)
        .chain(&lb.zip_entries)
        .filter(|c| commit::sha256_file(&c.dst).is_ok_and(|h| h != c.sha256))
        .map(|c| c.dst.clone())
        .collect()
}

/// Why the last batch can't be undone right now, if it can't.
pub fn blocker(lb: Option<&LastBatch>) -> Option<String> {
    let Some(lb) = lb else {
        return Some("Nothing to undo yet.".into());
    };
    if lb.undone {
        return Some("The last batch was already undone.".into());
    }
    if lb.in_progress {
        return Some("The last commit did not finish; check the files by hand.".into());
    }
    if lb.items.is_empty()
        && lb.created_files.is_empty()
        && lb.zip_entries.is_empty()
        && lb.zip.is_none()
        && lb.routed_zips.is_empty()
        && lb.renamed_dirs.is_empty()
    {
        return Some("The last batch filed nothing.".into());
    }
    for r in &lb.renamed_dirs {
        if !r.to.is_dir() {
            return Some(format!("{} is no longer there.", r.to.display()));
        }
        if r.from.exists() {
            return Some(format!(
                "{} exists again, so the folder can't get its old name back.",
                r.from.display()
            ));
        }
    }
    for item in &lb.items {
        if item.return_to.is_none()
            && !item.combined
            && item.merged.is_empty()
            && item.piece_of.is_none()
        {
            return Some(format!(
                "{} was added by hand and there is no inbox to put it back in.",
                item.src.display()
            ));
        }
        match std::fs::metadata(&item.dst) {
            Ok(m) if m.len() == item.bytes => {}
            Ok(_) => {
                return Some(format!(
                    "{} has changed since it was filed.",
                    item.dst.display()
                ));
            }
            Err(_) => {
                return Some(format!("{} is no longer there.", item.dst.display()));
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct UndoReport {
    /// (filed path, where it went back to or why not).
    pub outcomes: Vec<(PathBuf, Result<PathBuf, String>)>,
    pub warnings: Vec<String>,
    pub last_batch: LastBatch,
}

/// Undoes `lb`. Every step is logged as `undo`. Continues past individual failures.
pub fn run_undo(data_dir: &Path, mut lb: LastBatch, force_copy: bool) -> UndoReport {
    let log = oplog::path(data_dir);
    // Shared by the logging closure and the steps below.
    let warnings = std::cell::RefCell::new(Vec::new());
    let record =
        |src: Option<&Path>, dst: Option<&Path>, client: Option<&str>, err: Option<&str>| {
            let rec = OpRecord {
                ts: Utc::now(),
                batch: lb.batch,
                op: Op::Undo,
                client_id: client.map(str::to_string),
                src: src.map(Path::to_path_buf),
                dst: dst.map(Path::to_path_buf),
                bytes: None,
                sha256: None,
                result: if err.is_some() {
                    OpResult::Error
                } else {
                    OpResult::Ok
                },
                error: err.map(str::to_string),
                copy_of: None,
                note: None,
            };
            if let Err(e) = oplog::append(&log, &rec) {
                warnings
                    .borrow_mut()
                    .push(format!("ops.jsonl could not be written: {e}"));
            }
        };

    // 1. Files back where they came from (newest first).
    let mut outcomes = Vec::new();
    let mut piece_failed: Vec<PathBuf> = Vec::new();
    for item in lb.items.iter().rev() {
        // Copies to related clients first (P07a): removed only if unchanged since written.
        for c in &item.copies {
            match commit::sha256_file(&c.dst) {
                Ok(h) if h == c.sha256 => match std::fs::remove_file(&c.dst) {
                    Ok(()) => record(Some(&c.dst), None, Some(&c.client_id), None),
                    Err(e) => {
                        let msg = format!("cannot remove the copy: {e}");
                        record(Some(&c.dst), None, Some(&c.client_id), Some(msg.as_str()));
                        warnings
                            .borrow_mut()
                            .push(format!("{}: {msg}", c.dst.display()));
                    }
                },
                Ok(_) => {
                    let msg = "modified since filed, left in place";
                    record(Some(&c.dst), None, Some(&c.client_id), Some(msg));
                    warnings
                        .borrow_mut()
                        .push(format!("{}: {msg}", c.dst.display()));
                }
                Err(_) => {
                    let msg = "already gone";
                    record(Some(&c.dst), None, Some(&c.client_id), Some(msg));
                }
            }
        }
        if let Some(original) = &item.piece_of {
            // A piece of a split: delete it (the app made it). Its original is restored below
            // only if all its pieces are gone, so pages never end up in two places.
            let result = std::fs::remove_file(&item.dst)
                .map(|()| {
                    record(Some(&item.dst), None, Some(&item.client_id), None);
                    original.clone()
                })
                .map_err(|e| format!("cannot remove the piece: {e}"));
            if result.is_err() {
                piece_failed.push(original.clone());
            }
            outcomes.push((item.dst.clone(), result));
            continue;
        }
        if item.combined || !item.merged.is_empty() {
            // A combined PDF: put the parked originals back, then delete the combined file (the
            // app made it). If any original can't go back, the ones already moved are parked
            // again and the combined file stays, so nothing is filed twice.
            let mut back: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut problem = None;
            for m in &item.merged {
                let restore = restore_moved(m, force_copy);
                match restore {
                    Ok(target) => {
                        record(Some(&m.kept), Some(&target), Some(&item.client_id), None);
                        back.push((target, m.kept.clone()));
                    }
                    Err(e) => {
                        record(
                            Some(&m.kept),
                            Some(&m.original),
                            Some(&item.client_id),
                            Some(e.as_str()),
                        );
                        problem = Some(e);
                        break;
                    }
                }
            }
            let result = match problem {
                Some(e) => {
                    for (target, kept) in back.iter().rev() {
                        if let Err(e2) = commit::transfer(target, kept, force_copy) {
                            warnings.borrow_mut().push(format!(
                                "{} could not be parked again ({e2}); it may be filed twice",
                                target.display()
                            ));
                        }
                    }
                    Err(format!(
                        "the combined PDF was kept because an original could not go back: {e}"
                    ))
                }
                None => std::fs::remove_file(&item.dst)
                    .map(|()| {
                        record(Some(&item.dst), None, Some(&item.client_id), None);
                        item.merged
                            .first()
                            .map_or_else(|| item.dst.clone(), |m| m.original.clone())
                    })
                    .map_err(|e| format!("cannot remove the combined file: {e}")),
            };
            outcomes.push((item.dst.clone(), result));
            continue;
        }
        let result: Result<PathBuf, String> = (|| -> Result<PathBuf, String> {
            let target = item
                .return_to
                .clone()
                .ok_or_else(|| "no inbox to return it to".to_string())?;
            if let Some(parent) = target.parent()
                && !parent.is_dir()
            {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot recreate {}: {e}", parent.display()))?;
            }
            let target = commit::free_path(&target);
            commit::transfer(&item.dst, &target, force_copy)?;
            Ok(target)
        })();
        match &result {
            Ok(t) => record(Some(&item.dst), Some(t), Some(&item.client_id), None),
            Err(e) => record(
                Some(&item.dst),
                item.return_to.as_deref(),
                Some(&item.client_id),
                Some(e.as_str()),
            ),
        }
        outcomes.push((item.dst.clone(), result));
    }

    // 1b. Split originals back into the inbox, unless one of their pieces is still filed.
    for m in &lb.split_originals {
        if piece_failed.contains(&m.original) {
            warnings.borrow_mut().push(format!(
                "{} stays in {} because one of its pieces could not be removed",
                m.original.display(),
                m.kept.display()
            ));
            continue;
        }
        match restore_moved(m, force_copy) {
            Ok(target) => record(Some(&m.kept), Some(&target), None, None),
            Err(e) => {
                record(Some(&m.kept), Some(&m.original), None, Some(e.as_str()));
                warnings.borrow_mut().push(format!(
                    "{} could not be put back ({e}); it is still in {}",
                    m.original.display(),
                    m.kept.display()
                ));
            }
        }
    }
    if let Some(dir) = lb.split_originals.first().and_then(|m| m.kept.parent()) {
        let _ = std::fs::remove_dir(dir);
    }
    if let Err(e) = commit::remove_partial_batch(data_dir, lb.batch) {
        warnings
            .borrow_mut()
            .push(format!("partial_splits.json could not be updated: {e}"));
    }

    // 1b. Files unpacked from a zip (P07b): removed only if unchanged since written.
    for c in &lb.zip_entries {
        match commit::sha256_file(&c.dst) {
            Ok(h) if h == c.sha256 => match std::fs::remove_file(&c.dst) {
                Ok(()) => record(Some(&c.dst), None, Some(&c.client_id), None),
                Err(e) => {
                    let msg = format!("cannot remove {}: {e}", c.dst.display());
                    warnings.borrow_mut().push(msg.clone());
                    record(Some(&c.dst), None, Some(&c.client_id), Some(&msg));
                }
            },
            Ok(_) => {
                let msg = "modified since filed, left in place";
                record(Some(&c.dst), None, Some(&c.client_id), Some(msg));
                warnings
                    .borrow_mut()
                    .push(format!("{}: {msg}", c.dst.display()));
            }
            Err(_) => record(Some(&c.dst), None, Some(&c.client_id), Some("already gone")),
        }
    }

    // 2. Overwritten files back in place.
    for b in &lb.backups {
        let result = if b.original.exists() {
            Err(format!(
                "{} is occupied; the old file stays in {}",
                b.original.display(),
                b.kept.display()
            ))
        } else {
            commit::transfer(&b.kept, &b.original, force_copy).map(|_| ())
        };
        match &result {
            Ok(()) => record(Some(&b.kept), Some(&b.original), None, None),
            Err(e) => {
                warnings.borrow_mut().push(e.clone());
                record(Some(&b.kept), Some(&b.original), None, Some(e));
            }
        }
    }
    if let Some(dir) = lb.backups.first().and_then(|b| b.kept.parent()) {
        let _ = std::fs::remove_dir(dir);
    }

    // 3. Sentinels this batch wrote (files the app created).
    for f in &lb.created_files {
        if f.is_file() {
            match std::fs::remove_file(f) {
                Ok(()) => record(Some(f), None, None, None),
                Err(e) => {
                    let msg = format!("cannot remove {}: {e}", f.display());
                    warnings.borrow_mut().push(msg.clone());
                    record(Some(f), None, None, Some(&msg));
                }
            }
        }
    }

    // 4. Folders this batch created, deepest first, only if empty.
    let mut dirs = lb.created_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in &dirs {
        if d.is_dir() && std::fs::remove_dir(d).is_ok() {
            record(Some(d), None, None, None);
        }
    }

    // 5. The imported zips back where they were (P07b).
    for z in lb.zip.iter().chain(&lb.routed_zips) {
        let target = commit::free_path(&z.original);
        let result = match z.original.parent() {
            Some(parent) if !parent.is_dir() => std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot recreate {}: {e}", parent.display())),
            _ => Ok(()),
        }
        .and_then(|()| commit::transfer(&z.kept, &target, force_copy).map(|_| ()));
        match &result {
            Ok(()) => record(Some(&z.kept), Some(&target), None, None),
            Err(e) => {
                warnings.borrow_mut().push(format!(
                    "the zip could not be put back ({e}); it is kept at {}",
                    z.kept.display()
                ));
                record(Some(&z.kept), Some(&z.original), None, Some(e));
            }
        }
    }

    // 6. Client folders renamed to the template's name get their old names back (V9).
    for r in lb.renamed_dirs.iter().rev() {
        let result = if r.from.exists() {
            Err(format!("{} exists", r.from.display()))
        } else {
            std::fs::rename(&r.to, &r.from).map_err(|e| e.to_string())
        };
        match &result {
            Ok(()) => record(Some(&r.to), Some(&r.from), Some(&r.client_id), None),
            Err(e) => {
                let msg = format!("cannot rename {} back: {e}", r.to.display());
                warnings.borrow_mut().push(msg.clone());
                record(Some(&r.to), Some(&r.from), Some(&r.client_id), Some(&msg));
            }
        }
    }

    lb.undone = true;
    if let Err(e) = commit::save_last_batch(data_dir, &lb) {
        warnings
            .borrow_mut()
            .push(format!("last_batch.json could not be written: {e}"));
    }
    tracing::info!(
        batch = %lb.batch,
        back = outcomes.iter().filter(|o| o.1.is_ok()).count(),
        failed = outcomes.iter().filter(|o| o.1.is_err()).count(),
        "undo finished"
    );
    UndoReport {
        outcomes,
        warnings: warnings.into_inner(),
        last_batch: lb,
    }
}

/// Moves a parked file back to where it came from (recreating its folder, " (2)" if taken).
fn restore_moved(m: &commit::MovedOriginal, force_copy: bool) -> Result<PathBuf, String> {
    if let Some(parent) = m.original.parent()
        && !parent.is_dir()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot recreate {}: {e}", parent.display()))?;
    }
    let target = commit::free_path(&m.original);
    commit::transfer(&m.kept, &target, force_copy)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::ClientList;
    use crate::commit::{CommitInput, run_commit};
    use crate::config::ColumnMap;
    use crate::intake::{Kind, QueuedFile, Source};
    use crate::plan::{self, BatchSettings, CollisionChoice, PlanInput, Rescan, RowEdits};
    use crate::sentinel::{self, FolderBinding, FoldersFile, Status};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use uuid::Uuid;

    /// Every file and folder under `dir` with a hash of each file's content.
    fn snapshot(dir: &Path) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                if p.is_dir() {
                    out.insert(format!("{rel}/"), String::new());
                    walk(base, &p, out);
                } else {
                    out.insert(rel, commit::sha256_file(&p).unwrap());
                }
            }
        }
        walk(dir, dir, &mut out);
        out
    }

    struct World {
        _tmp: tempfile::TempDir,
        data: PathBuf,
        root: PathBuf,
        inbox: PathBuf,
        desk: PathBuf,
        clients: Arc<ClientList>,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root, inbox, desk) = (
            tmp.path().join("data"),
            tmp.path().join("root"),
            tmp.path().join("inbox"),
            tmp.path().join("desk"),
        );
        for d in [&data, &root, &inbox, &desk] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("c.csv");
        std::fs::write(
            &csv,
            "id,name\n1001,\"Smith, John\"\n1002,Acme LLC\n1004,Baker Farms\n",
        )
        .unwrap();
        let map = ColumnMap {
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec![],
        };
        let clients = Arc::new(
            crate::clients::apply_mapping(&crate::clients::read_csv(&csv).unwrap(), &map).unwrap(),
        );
        // Smith: bound, with an existing return that will be overwritten.
        let smith = root.join("Smith, John (1001)");
        std::fs::create_dir_all(smith.join("Tax Returns").join("2025")).unwrap();
        sentinel::write_sentinel(
            &root,
            &smith,
            clients.match_by_id("1001").unwrap(),
            Uuid::new_v4(),
        )
        .unwrap();
        std::fs::write(
            smith
                .join("Tax Returns")
                .join("2025")
                .join("2025 Tax Return - Smith, John.pdf"),
            b"old return",
        )
        .unwrap();
        // Acme: ID-named folder without sentinel (bind on commit). Baker: no folder.
        std::fs::create_dir_all(root.join("Acme LLC (1002)").join("Permanent")).unwrap();
        World {
            _tmp: tmp,
            data,
            root,
            inbox,
            desk,
            clients,
        }
    }

    fn files(w: &World) -> Vec<QueuedFile> {
        let mk = |dir: &Path, name: &str, body: &[u8], source: Source| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            QueuedFile {
                path: p,
                size: body.len() as u64,
                added: chrono::Local::now(),
                source,
                kind: Kind::Pdf,
            }
        };
        std::fs::create_dir_all(w.inbox.join("sub")).unwrap();
        vec![
            mk(
                &w.inbox,
                "Smith, John_1001_2025_1040.pdf",
                b"new return",
                Source::Inbox(w.inbox.clone()),
            ),
            mk(
                &w.inbox,
                "Smith, John_1001_2025 organizer.pdf",
                b"organizer",
                Source::Inbox(w.inbox.clone()),
            ),
            mk(
                &w.inbox.join("sub"),
                "Acme LLC_1002_2025_1120-S.pdf",
                b"acme",
                Source::Inbox(w.inbox.clone()),
            ),
            mk(
                &w.inbox,
                "Baker Farms_1004_2025_1065.pdf",
                b"baker",
                Source::Inbox(w.inbox.clone()),
            ),
            mk(
                &w.desk,
                "Baker Farms_1004_2025 K-1.pdf",
                b"k1",
                Source::Manual,
            ),
        ]
    }

    fn plan_input(w: &World, files: Vec<QueuedFile>) -> PlanInput {
        let folders = vec![
            FolderBinding {
                path: w.root.join("Smith, John (1001)"),
                client_id: Some("1001".into()),
                status: Status::Bound,
                score: None,
                note: None,
            },
            FolderBinding {
                path: w.root.join("Acme LLC (1002)"),
                client_id: Some("1002".into()),
                status: Status::UnboundNamed,
                score: None,
                note: None,
            },
        ];
        let mut edits = HashMap::new();
        edits.insert(
            files[0].path.clone(),
            RowEdits {
                collision: Some(CollisionChoice::Overwrite),
                ..Default::default()
            },
        );
        PlanInput {
            files,
            edits,
            batch: BatchSettings::default(),
            clients: Some(w.clients.clone()),
            rules: crate::rules::tax_rules(),
            template: crate::template::default_tree(),
            folders: Some(FoldersFile {
                root: w.root.clone(),
                scanned_at: Utc::now(),
                folders,
                ignored: vec![],
            }),
            root: Some(w.root.clone()),
            id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
            fuzzy_suggest: 0.85,
            fuzzy_auto: 0.97,
            current_year: 2026,
            sniff_enabled: false,
            sniff: HashMap::new(),
            rescan: Rescan::Done,
            partial: Vec::new(),
            relationships: Arc::default(),
        }
    }

    // ----- P07a: copies to related clients -----

    struct CopyWorld {
        _tmp: tempfile::TempDir,
        data: PathBuf,
        root: PathBuf,
        inbox: PathBuf,
        clients: Arc<ClientList>,
        rels: Arc<crate::relationships::Relationships>,
    }

    fn copy_world() -> CopyWorld {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root, inbox) = (
            tmp.path().join("data"),
            tmp.path().join("root"),
            tmp.path().join("inbox"),
        );
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("c.csv");
        std::fs::write(
            &csv,
            "id,name\n4472,Riverbend Holdings LP\n2093,\"Baker, Thomas\"\n1042,\"Hendricks, Mark\"\n4471,Whitfield Farms LLC\n1234,Entity Co\n",
        )
        .unwrap();
        let map = ColumnMap {
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec![],
        };
        let clients = Arc::new(
            crate::clients::apply_mapping(&crate::clients::read_csv(&csv).unwrap(), &map).unwrap(),
        );
        let mut rels = crate::relationships::Relationships::default();
        for (p, c, role) in [
            ("4472", "2093", "partner"),
            ("1234", "2093", "shareholder"),
            ("1234", "1042", "shareholder"),
            ("1234", "4471", "member"),
        ] {
            rels.add(crate::relationships::Relationship {
                parent_id: p.into(),
                child_id: c.into(),
                role: role.into(),
                note: String::new(),
            })
            .unwrap();
        }
        CopyWorld {
            _tmp: tmp,
            data,
            root,
            inbox,
            clients,
            rels: Arc::new(rels),
        }
    }

    fn copy_plan(
        cw: &CopyWorld,
        name: &str,
        body: &[u8],
        folders: Vec<FolderBinding>,
    ) -> plan::PlanOutput {
        let path = cw.inbox.join(name);
        std::fs::write(&path, body).unwrap();
        let file = QueuedFile {
            path,
            size: body.len() as u64,
            added: chrono::Local::now(),
            source: Source::Inbox(cw.inbox.clone()),
            kind: Kind::Pdf,
        };
        plan::build_plan(&PlanInput {
            files: vec![file],
            edits: HashMap::new(),
            batch: BatchSettings {
                year: Some(2025),
                ..Default::default()
            },
            clients: Some(cw.clients.clone()),
            rules: crate::rules::tax_rules(),
            template: crate::template::default_tree(),
            folders: Some(FoldersFile {
                root: cw.root.clone(),
                scanned_at: Utc::now(),
                folders,
                ignored: vec![],
            }),
            root: Some(cw.root.clone()),
            id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
            fuzzy_suggest: 0.85,
            fuzzy_auto: 0.97,
            current_year: 2026,
            sniff_enabled: false,
            sniff: HashMap::new(),
            rescan: Rescan::Done,
            partial: Vec::new(),
            relationships: cw.rels.clone(),
        })
    }

    fn copy_commit(cw: &CopyWorld, out: &plan::PlanOutput) -> crate::commit::CommitReport {
        let input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            crate::commit::CommitTarget {
                root: cw.root.clone(),
                first_inbox: Some(cw.inbox.clone()),
                data_dir: cw.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        run_commit(&input, &AtomicUsize::new(0))
    }

    /// Splits `name` (already in the inbox) into pieces with the given bytes, all for `client`,
    /// and plans + commits them with the original as their split (P09 on_split hooks).
    fn copy_split(
        cw: &CopyWorld,
        name: &str,
        pieces: &[&[u8]],
        client: &str,
        rules: Vec<crate::rules::Rule>,
        skip: &[usize],
    ) -> (plan::PlanOutput, crate::commit::CommitReport) {
        let original = cw.inbox.join(name);
        let tmp = cw.data.join(crate::pdf::TMP_DIR).join("s1");
        let mut files = Vec::new();
        let mut edits = HashMap::new();
        for (k, body) in pieces.iter().enumerate() {
            let d = tmp.join((k + 1).to_string());
            std::fs::create_dir_all(&d).unwrap();
            let path = d.join(name);
            std::fs::write(&path, body).unwrap();
            edits.insert(
                path.clone(),
                RowEdits {
                    client_id: Some(client.into()),
                    split_of: Some(original.clone()),
                    skip: skip.contains(&k),
                    ..Default::default()
                },
            );
            files.push(QueuedFile {
                path,
                size: body.len() as u64,
                added: chrono::Local::now(),
                source: Source::Manual,
                kind: Kind::Pdf,
            });
        }
        let out = plan::build_plan(&PlanInput {
            files: files.clone(),
            edits,
            batch: BatchSettings {
                year: Some(2025),
                ..Default::default()
            },
            clients: Some(cw.clients.clone()),
            rules,
            template: crate::template::default_tree(),
            folders: Some(FoldersFile {
                root: cw.root.clone(),
                scanned_at: Utc::now(),
                folders: sentinel::scan_root(
                    &sentinel::ScanInput {
                        root: cw.root.clone(),
                        clients: cw.clients.clone(),
                        id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
                        threshold: 0.85,
                        ignored: vec![],
                    },
                    &sentinel::Progress::default(),
                )
                .unwrap(),
                ignored: vec![],
            }),
            root: Some(cw.root.clone()),
            id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
            fuzzy_suggest: 0.85,
            fuzzy_auto: 0.97,
            current_year: 2026,
            sniff_enabled: false,
            sniff: HashMap::new(),
            rescan: Rescan::Done,
            partial: Vec::new(),
            relationships: cw.rels.clone(),
        });
        for (k, p) in out.proposals.iter().enumerate() {
            let want = if skip.contains(&k) {
                plan::RowStatus::Skipped
            } else {
                plan::RowStatus::Ready
            };
            assert_eq!(p.status, want, "{:?}", p.status);
        }
        let splits = vec![crate::commit::SplitCommit {
            original: crate::commit::CombinedOriginal::stamp(
                &original,
                Source::Inbox(cw.inbox.clone()),
            )
            .unwrap(),
            pieces: files.iter().map(|f| f.path.clone()).collect(),
            total: files.len(),
        }];
        let input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &splits,
            crate::commit::CommitTarget {
                root: cw.root.clone(),
                first_inbox: Some(cw.inbox.clone()),
                data_dir: cw.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        (out, report)
    }

    /// P09 on_split: Whole. Two pieces of a per-shareholder K-1: each piece goes to the entity,
    /// the shareholder gets the unsplit original once (its bytes, not a piece's); undo restores.
    #[test]
    fn split_whole_copy_sends_the_original_once() {
        let cw = copy_world();
        let name = "Riverbend_4472_K-1 Baker, Thomas.pdf";
        std::fs::write(cw.inbox.join(name), b"whole k1").unwrap();
        let (root0, inbox0) = (snapshot(&cw.root), snapshot(&cw.inbox));
        let (out, report) = copy_split(
            &cw,
            name,
            &[b"part one", b"part two"],
            "4472",
            crate::rules::tax_rules(),
            &[],
        );
        assert_eq!(out.proposals[0].copies.len(), 1);
        assert!(out.proposals[0].copies[0].whole_of.is_some());
        assert!(
            out.proposals[1].copies.is_empty(),
            "the original goes there once"
        );
        assert_eq!(
            out.proposals[1].copy_notes,
            ["the whole original already goes there (with an earlier piece or batch)"]
        );
        let copy = cw
            .root
            .join("Baker, Thomas (2093)")
            .join("Source Documents")
            .join("2025")
            .join("2025 K-1 from Riverbend Holdings LP.pdf");
        assert_eq!(std::fs::read(&copy).unwrap(), b"whole k1");
        let returns = cw
            .root
            .join("Riverbend Holdings LP (4472)")
            .join("Tax Returns")
            .join("2025");
        assert_eq!(
            std::fs::read(returns.join("2025 K-1 - Baker, Thomas.pdf")).unwrap(),
            b"part one"
        );
        assert_eq!(
            std::fs::read(returns.join("2025 K-1 - Baker, Thomas (2).pdf")).unwrap(),
            b"part two"
        );
        assert_eq!(report.split_done, vec![cw.inbox.join(name)]);
        let undo = run_undo(&cw.data, report.last_batch, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(snapshot(&cw.root), root0);
        assert_eq!(snapshot(&cw.inbox), inbox0, "the original is back");
    }

    /// A skipped first piece doesn't take the Whole copy with it: the second piece sends it.
    #[test]
    fn split_whole_copy_survives_a_skipped_first_piece() {
        let cw = copy_world();
        let name = "Riverbend_4472_K-1 Baker, Thomas.pdf";
        std::fs::write(cw.inbox.join(name), b"whole k1").unwrap();
        let (out, _) = copy_split(
            &cw,
            name,
            &[b"part one", b"part two"],
            "4472",
            crate::rules::tax_rules(),
            &[0],
        );
        assert!(out.proposals[1].copies[0].whole_of.is_some());
        let copy = cw
            .root
            .join("Baker, Thomas (2093)")
            .join("Source Documents")
            .join("2025")
            .join("2025 K-1 from Riverbend Holdings LP.pdf");
        assert_eq!(std::fs::read(&copy).unwrap(), b"whole k1");
    }

    /// A split filed in two batches sends the whole original once: the second batch sees it is
    /// already there and adds no " (2)" copy.
    #[test]
    fn split_whole_copy_is_not_repeated_in_a_later_batch() {
        let cw = copy_world();
        let name = "Riverbend_4472_K-1 Baker, Thomas.pdf";
        std::fs::write(cw.inbox.join(name), b"whole k1").unwrap();
        let rules = crate::rules::tax_rules;
        copy_split(
            &cw,
            name,
            &[b"part one", b"part two"],
            "4472",
            rules(),
            &[1],
        );
        let (out, _) = copy_split(&cw, name, &[b"part two"], "4472", rules(), &[]);
        assert!(
            out.proposals[0].copies.is_empty(),
            "{:?}",
            out.proposals[0].copies
        );
        let docs = cw
            .root
            .join("Baker, Thomas (2093)")
            .join("Source Documents")
            .join("2025");
        assert_eq!(
            std::fs::read(docs.join("2025 K-1 from Riverbend Holdings LP.pdf")).unwrap(),
            b"whole k1"
        );
        assert!(
            !docs
                .join("2025 K-1 from Riverbend Holdings LP (2).pdf")
                .exists()
        );
    }

    /// P09 on_split: Segment. Two pieces of a K-1 package whose copy says Segment (the seed says
    /// Whole, #44): each piece fans out to the three related clients with its own pages.
    #[test]
    fn split_segment_copies_go_with_each_piece() {
        let cw = copy_world();
        let name = "Entity Co_1234_2025 K-1 package.pdf";
        std::fs::write(cw.inbox.join(name), b"package").unwrap();
        let mut rules = crate::rules::tax_rules();
        rules[0].copies[0].on_split = crate::rules::OnSplit::Segment;
        let (out, _) = copy_split(&cw, name, &[b"pages 1-2", b"pages 3-4"], "1234", rules, &[]);
        for p in &out.proposals {
            assert_eq!(p.copies.len(), 3);
            assert!(p.copies.iter().all(|c| c.whole_of.is_none()));
        }
        let baker = cw
            .root
            .join("Baker, Thomas (2093)")
            .join("Source Documents")
            .join("2025");
        assert_eq!(
            std::fs::read(baker.join("2025 K-1 Package from Entity Co.pdf")).unwrap(),
            b"pages 1-2"
        );
        assert_eq!(
            std::fs::read(baker.join("2025 K-1 Package from Entity Co (2).pdf")).unwrap(),
            b"pages 3-4"
        );
    }

    /// The fresh-VM test kit (scripts/make_vm_kit.py, docs/VM_TEST.md) plans the way the
    /// checklist says with the default Firm rules, after Conflicts' Bind all and Rename to
    /// template name. Keeps the checklist honest when rules or matching change.
    #[test]
    fn vm_kit_plans_as_the_checklist_says() {
        let kit = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("vm-kit");
        let tmp = tempfile::tempdir().unwrap();
        let (root, inbox, data) = (
            tmp.path().join("root"),
            tmp.path().join("inbox"),
            tmp.path().join("data"),
        );
        std::fs::create_dir_all(&data).unwrap();
        fn copy_tree(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for e in std::fs::read_dir(from).unwrap().flatten() {
                let dst = to.join(e.file_name());
                if e.path().is_dir() {
                    copy_tree(&e.path(), &dst);
                } else {
                    std::fs::copy(e.path(), dst).unwrap();
                }
            }
        }
        copy_tree(&kit.join("root"), &root);
        copy_tree(&kit.join("inbox"), &inbox);
        let table = crate::clients::read_csv(&kit.join("clients.csv")).unwrap();
        let map = ColumnMap {
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec!["Partner".into()],
        };
        let clients = Arc::new(crate::clients::apply_mapping(&table, &map).unwrap());
        let rtable = crate::clients::read_csv(&kit.join("relationships.csv")).unwrap();
        let cols = crate::relationships::RelColumns::guess(&rtable.headers);
        let rows = crate::relationships::rows_from_csv(&rtable, &cols, &clients).unwrap();
        assert_eq!((rows.rows.len(), rows.blank, rows.unknown), (4, 0, 0));
        let mut rels = crate::relationships::Relationships::default();
        for r in rows.rows {
            rels.add(r).unwrap();
        }
        let scan = || {
            let mut f = sentinel::scan_root(
                &sentinel::ScanInput {
                    root: root.clone(),
                    clients: clients.clone(),
                    id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
                    threshold: 0.85,
                    ignored: vec![],
                },
                &sentinel::Progress::default(),
            )
            .unwrap();
            f.sort_by(|a, b| a.path.cmp(&b.path));
            f
        };
        // Conflicts: Acme by ID, Baker Farms by its exact name: Bind all (2).
        let folders = scan();
        let st: Vec<_> = folders
            .iter()
            .map(|b| (b.name(), b.status, b.client_id.clone()))
            .collect();
        assert_eq!(
            st,
            [
                (
                    "Acme LLC (1002)".to_string(),
                    Status::UnboundNamed,
                    Some("1002".to_string())
                ),
                (
                    "Baker Farms".to_string(),
                    Status::UnboundFuzzy,
                    Some("1004".to_string())
                ),
            ]
        );
        assert!(folders[1].score.unwrap() >= 0.9999, "exact name");
        let firm = Uuid::new_v4();
        for b in &folders {
            let c = clients
                .match_by_id(b.client_id.as_deref().unwrap())
                .unwrap();
            sentinel::write_sentinel(&root, &b.path, c, firm).unwrap();
        }
        // Rename to template name (1): Baker Farms → Baker Farms (1004).
        let r = sentinel::rename_folders(
            &data,
            &root,
            &[(
                "1004".into(),
                root.join("Baker Farms"),
                root.join("Baker Farms (1004)"),
            )],
        );
        assert_eq!((r.done.len(), r.failed.len()), (1, 0));
        let folders = scan();
        assert!(folders.iter().all(|b| b.status == Status::Bound));

        let mut files: Vec<QueuedFile> = std::fs::read_dir(&inbox)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pdf"))
            .map(|path| QueuedFile {
                size: std::fs::metadata(&path).unwrap().len(),
                path,
                added: chrono::Local::now(),
                source: Source::Inbox(inbox.clone()),
                kind: Kind::Pdf,
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let rules = crate::rules::firm_rules();
        let out = plan::build_plan(&PlanInput {
            files,
            edits: HashMap::new(),
            batch: BatchSettings::default(),
            clients: Some(clients.clone()),
            rules: rules.clone(),
            template: crate::template::default_tree(),
            folders: Some(FoldersFile {
                root: root.clone(),
                scanned_at: Utc::now(),
                folders,
                ignored: vec![],
            }),
            root: Some(root.clone()),
            id_pattern: crate::parse::DEFAULT_ID_PATTERN.into(),
            fuzzy_suggest: 0.85,
            fuzzy_auto: 0.97,
            current_year: 2026,
            sniff_enabled: false,
            sniff: HashMap::new(),
            rescan: Rescan::Done,
            partial: Vec::new(),
            relationships: Arc::new(rels),
        });
        let mut report = Vec::new();
        for p in &out.proposals {
            let name = p
                .file
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let dest = p
                .dest
                .as_ref()
                .map(|d| d.strip_prefix(&root).unwrap().display().to_string())
                .unwrap_or_default();
            report.push(format!(
                "{name} | {:?} | {} | {dest} | copies {} | sugg {:?}",
                p.status,
                p.rule_name,
                p.copies.len(),
                p.suggestion
                    .as_ref()
                    .map(|(s, c)| (format!("{s:.2}"), c.name.clone()))
            ));
        }
        let report = report.join("\n");
        eprintln!("{report}");
        let expect = [
            "Acme LLC_1002_IRS Correspondence 2025.pdf | Ready | IRS Correspondence | Acme LLC (1002)\\Correspondence\\2025\\Acme LLC_1002_IRS Correspondence 2025.pdf | copies 0",
            "Acme LLC_1002_Payroll 2025.pdf | Ready | Payroll | Acme LLC (1002)\\Payroll\\2025\\Acme LLC_1002_Payroll 2025 (2).pdf | copies 0",
            "Acme LLC_1002_notes.pdf | Ready | Catch-all | Acme LLC (1002)\\Not Yet Filed\\Acme LLC_1002_notes.pdf",
            "Baker Farms_1004_Tax Returns 2025 part 1.pdf | Ready | Tax Returns | Baker Farms (1004)\\Workpapers & Support\\2025\\Baker Farms_1004_Tax Returns 2025 part 1.pdf",
            "Baker Farms_1004_Tax Returns 2025 part 2.pdf | Ready | Tax Returns | Baker Farms (1004)\\Workpapers & Support\\2025\\Baker Farms_1004_Tax Returns 2025 part 2.pdf",
            "Baker Farms_1004_scan.pdf | Ready | Catch-all | Baker Farms (1004)\\Not Yet Filed\\Baker Farms_1004_scan.pdf",
            "Entity Co_1234_2025 K-1 package.pdf | Ready | K-1 package | Entity Co (1234)\\Income Tax\\2025\\2025 K-1 Package - Entity Co.pdf | copies 3",
            "Front desk_1001_2025 print run.pdf | Ready | Catch-all | Smith, John (1001)\\Not Yet Filed\\Front desk_1001_2025 print run.pdf",
            "Riverbend_4472_2025 K-1 Baker, Thomas.pdf | Ready | K-1 | Riverbend Holdings LP (4472)\\Income Tax\\2025\\2025 K-1 - Baker, Thomas.pdf | copies 1",
            "Smith, John_1001_Sales Tax Q1 2025.pdf | Ready | Sales Tax | Smith, John (1001)\\Sales Tax\\2025\\Smith, John_1001_Sales Tax Q1 2025.pdf",
            "Smith, John_1001_UltraTax 2025 1040.pdf | Ready | UltraTax | Smith, John (1001)\\Income Tax\\2025\\Smith, John_1001_UltraTax 2025 1040.pdf",
            "Smth Jon_9999_2025 W-2.pdf | NeedsClient |",
        ];
        assert_eq!(report.lines().count(), expect.len());
        for (line, want) in report.lines().zip(expect) {
            assert!(line.starts_with(want), "\n got: {line}\nwant: {want}");
        }
        let w2 = out.proposals.last().unwrap();
        let (score, who) = w2.suggestion.as_ref().unwrap();
        assert_eq!(
            (format!("{score:.2}").as_str(), who.name.as_str()),
            ("0.95", "Smith, John")
        );
        let ultra = &out.proposals[10];
        assert_eq!(
            ultra.next_year_dir.as_ref().unwrap(),
            &root
                .join("Smith, John (1001)")
                .join("Income Tax")
                .join("2026")
        );
        let k1 = &out.proposals[8];
        assert_eq!(
            k1.copies[0]
                .dest
                .as_ref()
                .unwrap()
                .strip_prefix(&root)
                .unwrap(),
            Path::new("Baker, Thomas (2093)")
                .join("Income Tax")
                .join("2025")
                .join("2025 K-1 from Riverbend Holdings LP.pdf")
        );
        // The print run splits into three clients (ID 2018 looks like a year), each piece's
        // first page picking its rule.
        let pdfium = crate::pdf::pdfium(&crate::pdf::find_pdfium(Path::new("")).unwrap()).unwrap();
        let texts =
            crate::pdf::page_texts(pdfium, &inbox.join("Front desk_1001_2025 print run.pdf"))
                .unwrap();
        let finder =
            crate::split::Finder::new(&clients, crate::parse::DEFAULT_ID_PATTERN, 0.85, 2026)
                .unwrap();
        let (_, segs) = crate::split::propose(&texts, &finder);
        let got: Vec<_> = segs
            .iter()
            .map(|s| (s.pages.clone(), s.client_id.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            got,
            [
                (0..2, "1001".to_string()),
                (2..4, "1002".into()),
                (4..6, "2018".into())
            ]
        );
        let compiled = crate::rules::CompiledRules::new(&rules).unwrap();
        let sniffed: Vec<_> = segs
            .iter()
            .map(|s| {
                compiled
                    .first_specific_match(&texts[s.pages.start])
                    .map(|r| r.name.clone())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(sniffed, ["UltraTax", "Payroll", "Financials"]);
        let scan_pdf =
            crate::pdf::page_texts(pdfium, &inbox.join("Baker Farms_1004_scan.pdf")).unwrap();
        assert!(
            scan_pdf.iter().all(|t| t.trim().is_empty()),
            "the scan has no text layer"
        );
        let zip = crate::zipimport::prescan(&inbox.join("Baker Farms docs.zip")).unwrap();
        assert_eq!(zip.importable().count(), 3, "Jan, Feb, notes.txt");
        let q = crate::zipimport::prescan(&kit.join("zips").join("Queue me.zip")).unwrap();
        assert_eq!(q.importable().count(), 4);
    }

    /// P07a Done-when: the per-shareholder K-1 lands with the entity as "2025 K-1 - Baker,
    /// Thomas.pdf" and one copy in Baker's Source Documents/2025; undo restores the start.
    #[test]
    fn k1_for_one_shareholder_files_and_undoes() {
        let cw = copy_world();
        let name = "Riverbend_4472_K-1 Baker, Thomas.pdf";
        std::fs::write(cw.inbox.join(name), b"k1 for baker").unwrap();
        let (root0, inbox0) = (snapshot(&cw.root), snapshot(&cw.inbox));
        let out = copy_plan(&cw, name, b"k1 for baker", vec![]);
        assert_eq!(
            out.proposals[0].status,
            plan::RowStatus::Ready,
            "{:?}",
            out.proposals[0].status
        );
        let report = copy_commit(&cw, &out);
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        let primary = cw
            .root
            .join("Riverbend Holdings LP (4472)")
            .join("Tax Returns")
            .join("2025")
            .join("2025 K-1 - Baker, Thomas.pdf");
        let copy = cw
            .root
            .join("Baker, Thomas (2093)")
            .join("Source Documents")
            .join("2025")
            .join("2025 K-1 from Riverbend Holdings LP.pdf");
        assert_eq!(std::fs::read(&primary).unwrap(), b"k1 for baker");
        assert_eq!(std::fs::read(&copy).unwrap(), b"k1 for baker");
        assert!(!cw.inbox.join(name).exists());
        let ops = oplog::read(&oplog::path(&cw.data)).unwrap().records;
        let c = ops.iter().find(|r| r.op == Op::Copy).unwrap();
        assert_eq!(
            (c.copy_of.as_deref(), c.client_id.as_deref()),
            (Some("4472"), Some("2093"))
        );
        assert!(c.sha256.is_some());

        let lb = commit::load_last_batch(&cw.data).unwrap();
        assert_eq!(summary(&lb).copies, 1);
        let undo = run_undo(&cw.data, lb, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(snapshot(&cw.root), root0, "root back to its starting hash");
        assert_eq!(snapshot(&cw.inbox), inbox0);
    }

    /// P07a Done-when: a K-1 package for 1234 with three shareholder relationships lands with the
    /// entity and in each shareholder's Source Documents/2025; undo restores the start.
    #[test]
    fn k1_package_fans_out_and_undoes() {
        let cw = copy_world();
        let name = "Entity Co_1234_2025 K-1 package.pdf";
        std::fs::write(cw.inbox.join(name), b"package").unwrap();
        let (root0, inbox0) = (snapshot(&cw.root), snapshot(&cw.inbox));
        let out = copy_plan(&cw, name, b"package", vec![]);
        assert_eq!(out.proposals[0].copies.len(), 3);
        let report = copy_commit(&cw, &out);
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        assert!(
            cw.root
                .join("Entity Co (1234)")
                .join("Tax Returns")
                .join("2025")
                .join("2025 K-1 Package - Entity Co.pdf")
                .exists()
        );
        for folder in [
            "Baker, Thomas (2093)",
            "Hendricks, Mark (1042)",
            "Whitfield Farms LLC (4471)",
        ] {
            let c = cw
                .root
                .join(folder)
                .join("Source Documents")
                .join("2025")
                .join("2025 K-1 Package from Entity Co.pdf");
            assert_eq!(std::fs::read(&c).unwrap(), b"package", "{folder}");
        }
        let lb = commit::load_last_batch(&cw.data).unwrap();
        assert_eq!(lb.items[0].copies.len(), 3);
        let undo = run_undo(&cw.data, lb, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(snapshot(&cw.root), root0);
        assert_eq!(snapshot(&cw.inbox), inbox0);
    }

    /// A copy that fails stops the file before the primary moves: the source stays, and copies
    /// already written for it are rolled back.
    #[test]
    fn failing_second_copy_rolls_back_the_first() {
        let cw = copy_world();
        // Hendricks (copy #2) has a bound folder with the target sub-folder already there.
        let hendricks = cw.root.join("Hendricks, Mark (1042)");
        let target_dir = hendricks.join("Source Documents").join("2025");
        std::fs::create_dir_all(&target_dir).unwrap();
        let folders = vec![FolderBinding {
            path: hendricks.clone(),
            client_id: Some("1042".into()),
            status: Status::Bound,
            score: None,
            note: None,
        }];
        let name = "Entity Co_1234_2025 K-1 package.pdf";
        let out = copy_plan(&cw, name, b"package", folders);
        assert_eq!(out.proposals[0].status, plan::RowStatus::Ready);
        // A file appears at copy #2's destination after Preview.
        let blocker_file = target_dir.join("2025 K-1 Package from Entity Co.pdf");
        std::fs::write(&blocker_file, b"another file").unwrap();
        let report = copy_commit(&cw, &out);
        let err = report.outcomes[0].result.clone().unwrap_err();
        assert!(err.contains("copy to Hendricks, Mark failed"), "{err}");
        assert_eq!(
            std::fs::read(cw.inbox.join(name)).unwrap(),
            b"package",
            "source untouched"
        );
        let first = cw
            .root
            .join("Baker, Thomas (2093)")
            .join("Source Documents")
            .join("2025")
            .join("2025 K-1 Package from Entity Co.pdf");
        assert!(!first.exists(), "copy #1 rolled back");
        assert_eq!(std::fs::read(&blocker_file).unwrap(), b"another file");
        let ops = oplog::read(&oplog::path(&cw.data)).unwrap().records;
        assert!(
            ops.iter()
                .any(|r| r.op == Op::Rollback && r.result == OpResult::Ok)
        );
    }

    /// Undo removes copies only if unchanged; a changed copy stays and is reported.
    #[test]
    fn undo_leaves_a_modified_copy_and_reports_it() {
        let cw = copy_world();
        let name = "Riverbend_4472_K-1 Baker, Thomas.pdf";
        let out = copy_plan(&cw, name, b"k1", vec![]);
        let report = copy_commit(&cw, &out);
        assert!(report.outcomes[0].result.is_ok());
        let lb = commit::load_last_batch(&cw.data).unwrap();
        let copy = lb.items[0].copies[0].dst.clone();
        std::fs::write(&copy, b"annotated by the shareholder").unwrap();
        assert_eq!(modified_copies(&lb), vec![copy.clone()]);
        assert_eq!(
            blocker(Some(&lb)),
            None,
            "a modified copy does not block undo"
        );
        let undo = run_undo(&cw.data, lb, false);
        assert!(
            undo.warnings
                .iter()
                .any(|w| w.contains("modified since filed")),
            "{:?}",
            undo.warnings
        );
        assert_eq!(
            std::fs::read(&copy).unwrap(),
            b"annotated by the shareholder"
        );
        assert_eq!(
            std::fs::read(cw.inbox.join(name)).unwrap(),
            b"k1",
            "primary back"
        );
    }

    /// P07b Mode A collisions: Skip leaves the existing file, Overwrite replaces it (the old one
    /// kept for undo); undo puts the old file back byte for byte.
    #[test]
    fn zip_unpack_skip_and_overwrite_then_undo() {
        let cw = copy_world();
        let zip = cw.inbox.join("redo.zip");
        crate::zipimport::tests::make_zip(
            &zip,
            &[("Jan.pdf", b"new jan"), ("Feb.pdf", b"new feb")],
        );
        let baker = cw.root.join("Baker, Thomas (2093)");
        let dest_dir = baker.join("Permanent");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(dest_dir.join("Jan.pdf"), b"old jan").unwrap();
        std::fs::write(dest_dir.join("Feb.pdf"), b"old feb").unwrap();
        let (root0, inbox0) = (snapshot(&cw.root), snapshot(&cw.inbox));
        let scan = crate::zipimport::prescan(&zip).unwrap();
        let entries = scan
            .importable()
            .map(|e| {
                let rel = e.path.clone().unwrap();
                crate::commit::UnpackEntry {
                    index: e.index,
                    dest: dest_dir.join(&rel),
                    choice: if rel == "Jan.pdf" {
                        CollisionChoice::Skip
                    } else {
                        CollisionChoice::Overwrite
                    },
                    rel,
                }
            })
            .collect();
        let input = crate::commit::UnpackInput {
            zip: zip.clone(),
            client: cw.clients.match_by_id("2093").unwrap().clone(),
            folder: crate::plan::FolderAction::Exists(baker.clone()),
            dest_dir: dest_dir.clone(),
            entries,
            left_out: String::new(),
            target: crate::commit::CommitTarget {
                root: cw.root.clone(),
                first_inbox: Some(cw.inbox.clone()),
                data_dir: cw.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        };
        let report = crate::commit::run_unpack(&input, &AtomicUsize::new(0));
        assert!(
            report.stopped.is_none() && report.errors.is_empty(),
            "{report:?}"
        );
        assert_eq!((report.written, report.skipped), (1, 1));
        assert_eq!(
            std::fs::read(dest_dir.join("Jan.pdf")).unwrap(),
            b"old jan",
            "skipped"
        );
        assert_eq!(
            std::fs::read(dest_dir.join("Feb.pdf")).unwrap(),
            b"new feb",
            "overwritten"
        );
        let undo = run_undo(&cw.data, report.last_batch, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(snapshot(&cw.root), root0, "old Feb.pdf restored");
        assert_eq!(snapshot(&cw.inbox), inbox0, "zip back");
    }

    /// P07b Mode B on the Done-when fixture: nested folders flatten, two entries with the same
    /// name get " (2)", junk / .exe / ../escape.pdf are not extracted.
    #[test]
    fn zip_send_to_queue_flattens_with_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("Q4 scans.zip");
        crate::zipimport::tests::make_zip(
            &zip,
            &[
                ("2024/Bank/Jan.pdf", b"bank jan"),
                ("2025/Bank/Jan.pdf", b"other jan"),
                ("2024/notes.txt", b"notes"),
                ("__MACOSX/2024/._Jan.pdf", b"x"),
                ("Thumbs.db", b"x"),
                ("tools/setup.exe", b"MZ"),
                ("../escape.pdf", b"evil"),
            ],
        );
        let scan = crate::zipimport::prescan(&zip).unwrap();
        let tmp = dir.path().join("data").join("tmp").join("zip-1");
        let files = crate::zipimport::extract_flat(&scan, &tmp).unwrap();
        let mut names: Vec<(String, String)> = files
            .iter()
            .map(|(p, entry, _)| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    entry.clone(),
                )
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ("Jan (2).pdf".to_string(), "2025/Bank/Jan.pdf".to_string()),
                ("Jan.pdf".into(), "2024/Bank/Jan.pdf".into()),
                ("notes.txt".into(), "2024/notes.txt".into()),
            ]
        );
        assert_eq!(
            std::fs::read(tmp.join("Jan (2).pdf")).unwrap(),
            b"other jan"
        );
        assert_eq!(
            std::fs::read_dir(&tmp).unwrap().count(),
            3,
            "nothing else written"
        );
        assert!(!dir.path().join("data").join("escape.pdf").exists());
    }

    /// P07b Done-when, Unpack: a zip with nested folders, junk files, one .exe and one
    /// ../escape.pdf unpacks into the client folder with the right skips, and undo restores the
    /// tree and the zip byte for byte.
    #[test]
    fn zip_unpack_skips_bad_entries_and_undoes() {
        let cw = copy_world();
        let zip = cw.inbox.join("Q4 scans.zip");
        crate::zipimport::tests::make_zip(
            &zip,
            &[
                ("2024/Bank/Jan.pdf", b"jan"),
                ("2024/Bank/Feb.pdf", b"feb"),
                ("2024/notes.txt", b"notes"),
                ("__MACOSX/2024/._Jan.pdf", b"x"),
                ("Thumbs.db", b"x"),
                ("tools/setup.exe", b"MZ"),
                ("../escape.pdf", b"evil"),
            ],
        );
        // Baker's folder exists and is bound; one file is already there (Keep both).
        let baker = cw.root.join("Baker, Thomas (2093)");
        std::fs::create_dir_all(baker.join("Permanent").join("2024").join("Bank")).unwrap();
        std::fs::write(
            baker
                .join("Permanent")
                .join("2024")
                .join("Bank")
                .join("Feb.pdf"),
            b"old feb",
        )
        .unwrap();
        let (root0, inbox0) = (snapshot(&cw.root), snapshot(&cw.inbox));

        let scan = crate::zipimport::prescan(&zip).unwrap();
        assert!(scan.refused.is_none());
        let dest_dir = baker.join("Permanent");
        let entries: Vec<crate::commit::UnpackEntry> = scan
            .importable()
            .map(|e| {
                let rel = e.path.clone().unwrap();
                crate::commit::UnpackEntry {
                    index: e.index,
                    dest: dest_dir.join(rel.replace('/', "\\")),
                    rel,
                    choice: CollisionChoice::KeepBoth,
                }
            })
            .collect();
        assert_eq!(
            entries.len(),
            3,
            "junk, the .exe and ../escape.pdf are left out"
        );
        let input = crate::commit::UnpackInput {
            zip: zip.clone(),
            client: cw.clients.match_by_id("2093").unwrap().clone(),
            folder: crate::plan::FolderAction::Exists(baker.clone()),
            dest_dir: dest_dir.clone(),
            entries,
            left_out: "2 junk · 1 blocked · 1 unsafe".into(),
            target: crate::commit::CommitTarget {
                root: cw.root.clone(),
                first_inbox: Some(cw.inbox.clone()),
                data_dir: cw.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        };
        let report = crate::commit::run_unpack(&input, &AtomicUsize::new(0));
        assert!(
            report.stopped.is_none() && report.errors.is_empty(),
            "{report:?}"
        );
        assert_eq!((report.written, report.skipped), (3, 0));
        let bank = dest_dir.join("2024").join("Bank");
        assert_eq!(std::fs::read(bank.join("Jan.pdf")).unwrap(), b"jan");
        assert_eq!(
            std::fs::read(bank.join("Feb (2).pdf")).unwrap(),
            b"feb",
            "kept both"
        );
        assert_eq!(std::fs::read(bank.join("Feb.pdf")).unwrap(), b"old feb");
        assert_eq!(
            std::fs::read(dest_dir.join("2024").join("notes.txt")).unwrap(),
            b"notes"
        );
        assert!(!dest_dir.join("tools").exists() && !cw.root.join("escape.pdf").exists());
        assert!(report.zip_deleted && !zip.exists());
        let lb = report.last_batch;
        let kept = lb.zip.as_ref().unwrap().kept.clone();
        assert!(kept.starts_with(cw.data.join("tmp").join("undo")) && kept.exists());
        let ops = oplog::read(&oplog::path(&cw.data)).unwrap().records;
        assert_eq!(ops.iter().filter(|r| r.op == Op::ZipEntry).count(), 3);
        let imp = ops.iter().find(|r| r.op == Op::ZipImport).unwrap();
        assert!(
            imp.note
                .as_deref()
                .unwrap()
                .starts_with("unpack · 3 written"),
            "{imp:?}"
        );

        assert_eq!(blocker(Some(&lb)), None);
        let undo = run_undo(&cw.data, lb, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(snapshot(&cw.root), root0, "root restored");
        assert_eq!(snapshot(&cw.inbox), inbox0, "the zip is back");
    }

    fn commit(w: &World, force_copy: bool) -> crate::commit::CommitReport {
        let out = plan::build_plan(&plan_input(w, files(w)));
        assert!(
            out.proposals
                .iter()
                .all(|p| p.status == plan::RowStatus::Ready),
            "{:#?}",
            out.proposals
                .iter()
                .map(|p| (&p.file.path, &p.status))
                .collect::<Vec<_>>()
        );
        let mut input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            crate::commit::CommitTarget {
                root: w.root.clone(),
                first_inbox: Some(w.inbox.clone()),
                data_dir: w.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        input.force_copy = force_copy;
        run_commit(&input, &AtomicUsize::new(0))
    }

    fn round_trip(force_copy: bool) {
        let w = world();
        // Write the inputs once so the "before" snapshot includes them.
        let _ = files(&w);
        let root_before = snapshot(&w.root);
        let inbox_before = snapshot(&w.inbox);

        let report = commit(&w, force_copy);
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.folders_changed);
        let lb = commit::load_last_batch(&w.data).unwrap();
        assert!(!lb.in_progress);
        assert_eq!(lb.items.len(), 5);
        assert_eq!(lb.backups.len(), 1, "the old Smith return was kept");
        let smith_return = w
            .root
            .join("Smith, John (1001)")
            .join("Tax Returns")
            .join("2025")
            .join("2025 Tax Return - Smith, John.pdf");
        assert_eq!(std::fs::read(&smith_return).unwrap(), b"new return");
        assert!(
            sentinel::read_sentinel(&w.root.join("Acme LLC (1002)"))
                .unwrap()
                .is_some(),
            "bound"
        );
        let baker = w.root.join("Baker Farms (1004)");
        assert!(
            baker.join("Permanent").is_dir(),
            "created from the template"
        );
        assert!(sentinel::read_sentinel(&baker).unwrap().is_some());
        assert!(
            !w.desk.join("Baker Farms_1004_2025 K-1.pdf").exists(),
            "hand-added file moved (X53)"
        );
        let ops = oplog::read(&oplog::path(&w.data)).unwrap();
        assert!(ops.bad_lines.is_empty());
        let moves = ops
            .records
            .iter()
            .filter(|r| matches!(r.op, Op::Move | Op::CopyVerifyDelete) && r.result == OpResult::Ok)
            .count();
        assert_eq!(moves, 5);
        if force_copy {
            assert!(
                ops.records
                    .iter()
                    .any(|r| r.op == Op::CopyVerifyDelete && r.sha256.is_some())
            );
        }

        assert_eq!(blocker(Some(&lb)), None);
        assert_eq!(
            summary(&lb),
            UndoSummary {
                files: 5,
                restored: 1,
                folders: lb.created_dirs.len(),
                copies: 0,
            }
        );
        let undo = run_undo(&w.data, lb, force_copy);
        assert!(
            undo.outcomes.iter().all(|o| o.1.is_ok()),
            "{:?}",
            undo.outcomes
        );
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);

        // Root exactly as before. The inbox is too, except the hand-added file came back to the
        // inbox (X76) instead of the desk.
        assert_eq!(snapshot(&w.root), root_before);
        let mut inbox_after = snapshot(&w.inbox);
        inbox_after
            .remove("Baker Farms_1004_2025 K-1.pdf")
            .expect("hand-added file in the inbox");
        assert_eq!(
            std::fs::read(w.inbox.join("Baker Farms_1004_2025 K-1.pdf")).unwrap(),
            b"k1"
        );
        assert_eq!(inbox_after, inbox_before);
        assert_eq!(
            blocker(commit::load_last_batch(&w.data).as_ref()).as_deref(),
            Some("The last batch was already undone.")
        );

        // And it can be committed again.
        std::fs::remove_file(w.inbox.join("Baker Farms_1004_2025 K-1.pdf")).unwrap();
        let again = commit(&w, force_copy);
        assert!(
            again.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            again.outcomes
        );
    }

    #[test]
    fn commit_undo_commit_round_trip_same_volume() {
        round_trip(false);
    }

    #[test]
    fn commit_undo_commit_round_trip_copy_verify() {
        round_trip(true);
    }

    #[test]
    fn undo_is_blocked_when_a_filed_file_changed() {
        let w = world();
        let report = commit(&w, false);
        let lb = report.last_batch;
        std::fs::write(&lb.items[0].dst, b"edited after filing").unwrap();
        assert!(blocker(Some(&lb)).unwrap().contains("changed"));
        std::fs::remove_file(&lb.items[0].dst).unwrap();
        assert!(blocker(Some(&lb)).unwrap().contains("no longer there"));
        assert_eq!(blocker(None).as_deref(), Some("Nothing to undo yet."));
    }

    /// V6: a folder picked by hand below the client folder is created on commit and removed by
    /// undo once empty again.
    #[test]
    fn picked_new_subfolder_is_created_and_undone() {
        let w = world();
        let files = files(&w);
        let root_before = snapshot(&w.root);
        let mut inp = plan_input(&w, files);
        let first = inp.files[0].path.clone();
        inp.edits.insert(
            first.clone(),
            RowEdits {
                dest_rel: Some(PathBuf::from("Permanent").join("New Folder")),
                ..Default::default()
            },
        );
        let out = plan::build_plan(&inp);
        let p = out.proposals.iter().find(|p| p.file.path == first).unwrap();
        assert_eq!(p.status, plan::RowStatus::Ready, "{:?}", p.status);
        let dest = p.dest.clone().unwrap();
        assert!(
            dest.parent()
                .unwrap()
                .ends_with(Path::new("Permanent").join("New Folder"))
        );
        let input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            crate::commit::CommitTarget {
                root: w.root.clone(),
                first_inbox: Some(w.inbox.clone()),
                data_dir: w.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        assert!(dest.is_file());
        let lb = commit::load_last_batch(&w.data).unwrap();
        let undo = run_undo(&w.data, lb, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert!(!dest.parent().unwrap().exists(), "the new folder is gone");
        assert_eq!(snapshot(&w.root), root_before);
    }

    /// V11: a Firm rule with year folders also creates next year's, empty; undo removes it.
    #[test]
    fn next_year_folder_is_created_and_undone() {
        let w = world();
        let root_before = snapshot(&w.root);
        let path = w.inbox.join("Smith, John_1001_UltraTax 2025.pdf");
        std::fs::write(&path, b"return").unwrap();
        let file = QueuedFile {
            path: path.clone(),
            size: 6,
            added: chrono::Local::now(),
            source: Source::Inbox(w.inbox.clone()),
            kind: Kind::Pdf,
        };
        let mut inp = plan_input(&w, vec![file]);
        inp.rules = crate::rules::firm_rules();
        let out = plan::build_plan(&inp);
        let p = &out.proposals[0];
        assert_eq!(p.status, plan::RowStatus::Ready, "{:?}", p.status);
        let dest = p.dest.clone().unwrap();
        assert_eq!(
            dest.file_name().unwrap(),
            "Smith, John_1001_UltraTax 2025.pdf"
        );
        let next = dest.parent().unwrap().with_file_name("2026");
        assert_eq!(p.next_year_dir.as_ref(), Some(&next));
        let input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            crate::commit::CommitTarget {
                root: w.root.clone(),
                first_inbox: Some(w.inbox.clone()),
                data_dir: w.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        assert!(dest.is_file());
        assert!(
            next.is_dir(),
            "next year folder made: {:?}",
            report.warnings
        );
        let lb = commit::load_last_batch(&w.data).unwrap();
        let undo = run_undo(&w.data, lb, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert!(!next.exists());
        assert_eq!(snapshot(&w.root), root_before);
        assert!(path.is_file(), "back in the inbox");
    }

    #[test]
    fn failed_file_does_not_stop_the_batch() {
        let w = world();
        let out = plan::build_plan(&plan_input(&w, files(&w)));
        let mut input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &[],
            crate::commit::CommitTarget {
                root: w.root.clone(),
                first_inbox: Some(w.inbox.clone()),
                data_dir: w.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        // The organizer vanishes between Preview and Commit.
        std::fs::remove_file(w.inbox.join("Smith, John_1001_2025 organizer.pdf")).unwrap();
        let report = run_commit(&input, &AtomicUsize::new(0));
        let failed: Vec<_> = report
            .outcomes
            .iter()
            .filter(|o| o.result.is_err())
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(
            report.outcomes.iter().filter(|o| o.result.is_ok()).count(),
            4
        );
        let ops = oplog::read(&oplog::path(&w.data)).unwrap();
        assert!(
            ops.records
                .iter()
                .any(|r| r.result == OpResult::Error && r.error.is_some())
        );
        // A destination that appeared after Preview is never overwritten.
        input.batch = uuid::Uuid::new_v4();
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_err()),
            "everything is already filed"
        );
    }

    const BAKER: [&str; 3] = [
        "Baker Farms_1004_2025_1065.pdf",
        "Baker Farms_1004_2025 page2.pdf",
        "Baker Farms_1004_2025 page3.pdf",
    ];

    /// Three Baker PDFs (1, 2, 3 pages) in `dir`, merged into data/tmp, planned, and turned into
    /// a commit input. `first_inbox` is what the commit is told.
    fn combined_input(
        w: &World,
        dir: &Path,
        source: Source,
        first_inbox: Option<PathBuf>,
    ) -> (Vec<PathBuf>, CommitInput) {
        let paths: Vec<PathBuf> = BAKER
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let p = dir.join(n);
                let pages: Vec<String> = (0..=i).map(|k| format!("{n} p{k}")).collect();
                let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
                crate::pdf::write_test_pdf(&p, &refs);
                p
            })
            .collect();
        let tmp = w
            .data
            .join(crate::pdf::TMP_DIR)
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&tmp).unwrap();
        let merged = tmp.join(BAKER[0]);
        let inputs: Vec<(PathBuf, String)> = paths
            .iter()
            .map(|p| {
                (
                    p.clone(),
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(crate::pdf::merge_pdfs(&inputs, &merged).unwrap(), 6);
        let file = QueuedFile {
            path: merged.clone(),
            size: std::fs::metadata(&merged).unwrap().len(),
            added: chrono::Local::now(),
            source: Source::Manual,
            kind: Kind::Pdf,
        };
        let mut inp = plan_input(w, vec![file]);
        inp.edits.insert(
            merged.clone(),
            RowEdits {
                client_id: Some("1004".into()),
                ..Default::default()
            },
        );
        let out = plan::build_plan(&inp);
        assert_eq!(
            out.proposals[0].status,
            plan::RowStatus::Ready,
            "{:?}",
            out.proposals[0].status
        );
        let mut groups = HashMap::new();
        groups.insert(
            merged,
            paths
                .iter()
                .map(|p| crate::commit::CombinedOriginal::stamp(p, source.clone()).unwrap())
                .collect(),
        );
        let input = CommitInput::from_plan(
            &out.proposals,
            &groups,
            &[],
            crate::commit::CommitTarget {
                root: w.root.clone(),
                first_inbox,
                data_dir: w.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        (paths, input)
    }

    #[test]
    fn combined_pdf_round_trip() {
        let w = world();
        let root_before = snapshot(&w.root);
        let (paths, input) = combined_input(
            &w,
            &w.inbox,
            Source::Inbox(w.inbox.clone()),
            Some(w.inbox.clone()),
        );
        let inbox_before = snapshot(&w.inbox);
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.unparked.is_empty());
        let dest = report.outcomes[0].result.clone().unwrap();
        assert_eq!(
            crate::pdf::page_count(&dest).unwrap(),
            6,
            "Done-when: correct page count"
        );
        assert_eq!(
            dest.file_name().unwrap(),
            "2025 Tax Return - Baker Farms.pdf"
        );
        let combined = w.inbox.join(crate::intake::COMBINED_DIR);
        for p in &paths {
            assert!(!p.exists(), "{} left the inbox", p.display());
            assert!(
                combined.join(p.file_name().unwrap()).exists(),
                "parked in _combined"
            );
        }
        assert!(
            !input.items[0].src.parent().unwrap().exists(),
            "temp folder removed"
        );
        let ops = oplog::read(&oplog::path(&w.data)).unwrap();
        assert_eq!(ops.records.iter().filter(|r| r.op == Op::Merge).count(), 3);

        let lb = report.last_batch;
        assert_eq!(blocker(Some(&lb)), None);
        let undo = run_undo(&w.data, lb, false);
        assert!(
            undo.outcomes.iter().all(|o| o.1.is_ok()),
            "{:?}",
            undo.outcomes
        );
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(
            snapshot(&w.root),
            root_before,
            "combined file and new folder gone"
        );
        assert_eq!(
            snapshot(&w.inbox),
            inbox_before,
            "Done-when: undo restores the originals"
        );
    }

    #[test]
    fn original_that_cannot_be_parked_is_reported_not_refiled() {
        use std::os::windows::fs::OpenOptionsExt;
        let w = world();
        let (paths, input) = combined_input(
            &w,
            &w.inbox,
            Source::Inbox(w.inbox.clone()),
            Some(w.inbox.clone()),
        );
        // A viewer has page2 open without delete sharing: it can't be moved.
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&paths[1])
            .unwrap();
        let report = run_commit(&input, &AtomicUsize::new(0));
        drop(hold);
        assert!(
            report.outcomes[0].result.is_ok(),
            "the combined PDF is filed"
        );
        assert_eq!(report.unparked, vec![paths[1].clone()]);
        assert!(report.warnings.iter().any(|w| w.contains("marked Skip")));
        assert!(paths[1].exists() && !paths[0].exists() && !paths[2].exists());

        // Undo is allowed (not the "added by hand" message) and restores what was parked.
        let lb = report.last_batch;
        assert!(lb.items[0].combined);
        assert_eq!(blocker(Some(&lb)), None);
        let undo = run_undo(&w.data, lb, false);
        assert!(
            undo.outcomes.iter().all(|o| o.1.is_ok()),
            "{:?}",
            undo.outcomes
        );
        assert!(paths.iter().all(|p| p.exists()));
    }

    #[test]
    fn original_changed_after_combining_blocks_that_row() {
        let w = world();
        let (paths, input) = combined_input(
            &w,
            &w.inbox,
            Source::Inbox(w.inbox.clone()),
            Some(w.inbox.clone()),
        );
        crate::pdf::write_test_pdf(&paths[1].with_extension("tmp.pdf"), &["corrected"]);
        std::fs::remove_file(&paths[1]).unwrap();
        std::fs::rename(paths[1].with_extension("tmp.pdf"), &paths[1]).unwrap();
        let report = run_commit(&input, &AtomicUsize::new(0));
        let err = report.outcomes[0].result.clone().unwrap_err();
        assert!(err.contains("changed"), "{err}");
        assert!(paths.iter().all(|p| p.exists()), "nothing parked");
        assert!(!w.root.join("Baker Farms (1004)").exists(), "nothing filed");
    }

    #[test]
    fn without_an_inbox_hand_added_originals_park_in_data() {
        let w = world();
        let (paths, input) = combined_input(&w, &w.desk, Source::Manual, None);
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(
            !w.desk.join(crate::intake::COMBINED_DIR).exists(),
            "nothing created next to the files"
        );
        let parked = w
            .data
            .join(crate::commit::COMBINED_DATA_DIR)
            .join(input.batch.to_string());
        for p in &paths {
            assert!(parked.join(p.file_name().unwrap()).exists());
        }
    }

    #[test]
    fn undo_of_combined_pdf_is_all_or_nothing() {
        use std::os::windows::fs::OpenOptionsExt;
        let w = world();
        let (paths, input) = combined_input(
            &w,
            &w.inbox,
            Source::Inbox(w.inbox.clone()),
            Some(w.inbox.clone()),
        );
        let report = run_commit(&input, &AtomicUsize::new(0));
        let dest = report.outcomes[0].result.clone().unwrap();
        let lb = report.last_batch;
        // The third parked original is held open: it can't go back.
        let kept3 = lb.items[0].merged[2].kept.clone();
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&kept3)
            .unwrap();
        let undo = run_undo(&w.data, lb, false);
        drop(hold);
        assert!(undo.outcomes[0].1.is_err());
        assert!(dest.exists(), "combined PDF kept");
        assert!(
            paths.iter().all(|p| !p.exists()),
            "originals parked again, not half restored"
        );
        let combined = w.inbox.join(crate::intake::COMBINED_DIR);
        assert_eq!(std::fs::read_dir(&combined).unwrap().count(), 3);
    }

    /// A 12-page print run naming clients 1001, 1002 and 1004 (with a cover page).
    fn print_run(dir: &Path) -> PathBuf {
        let p = dir.join("Smith, John_1001_2025 print run.pdf");
        let pages = [
            "Cover sheet",
            "Client 1001 Form 1040",
            "page 2",
            "page 3",
            "Client 1002 Form 1120-S",
            "schedule K",
            "continued",
            "statement",
            "Client 1004 Form 1065",
            "page 2",
            "page 3",
            "end",
        ];
        crate::pdf::write_test_pdf(&p, &pages);
        p
    }

    /// Splits `original` as proposed, plans the pieces and builds the commit input. `keep`
    /// limits which pieces go into the commit (by index).
    fn split_input(
        w: &World,
        original: &Path,
        keep: &[usize],
    ) -> (Vec<crate::split::Segment>, CommitInput) {
        let pdfium = crate::pdf::pdfium(&crate::pdf::find_pdfium(Path::new("")).unwrap()).unwrap();
        let texts = crate::pdf::page_texts(pdfium, original).unwrap();
        let finder =
            crate::split::Finder::new(&w.clients, crate::parse::DEFAULT_ID_PATTERN, 0.9, 2026)
                .unwrap();
        let (_, segments) = crate::split::propose(&texts, &finder);
        let tmp = w.data.join(crate::pdf::TMP_DIR).join("s1");
        let outs: Vec<PathBuf> = (1..=segments.len())
            .map(|k| {
                let d = tmp.join(k.to_string());
                std::fs::create_dir_all(&d).unwrap();
                d.join(original.file_name().unwrap())
            })
            .collect();
        let ranges: Vec<_> = segments.iter().map(|s| s.pages.clone()).collect();
        crate::pdf::split_pdf(original, &ranges, &outs).unwrap();
        let stamp =
            crate::commit::CombinedOriginal::stamp(original, Source::Inbox(w.inbox.clone()))
                .unwrap();
        let files: Vec<QueuedFile> = keep
            .iter()
            .map(|&i| QueuedFile {
                path: outs[i].clone(),
                size: std::fs::metadata(&outs[i]).unwrap().len(),
                added: chrono::Local::now(),
                source: Source::Manual,
                kind: Kind::Pdf,
            })
            .collect();
        let mut inp = plan_input(w, files);
        inp.edits.clear();
        for &i in keep {
            inp.edits.insert(
                outs[i].clone(),
                RowEdits {
                    client_id: segments[i].client_id.clone(),
                    ..Default::default()
                },
            );
        }
        let out = plan::build_plan(&inp);
        for p in &out.proposals {
            assert_eq!(p.status, plan::RowStatus::Ready, "{:?}", p.status);
        }
        let splits = vec![crate::commit::SplitCommit {
            original: stamp,
            pieces: outs.clone(),
            total: outs.len(),
        }];
        let input = CommitInput::from_plan(
            &out.proposals,
            &HashMap::new(),
            &splits,
            crate::commit::CommitTarget {
                root: w.root.clone(),
                first_inbox: Some(w.inbox.clone()),
                data_dir: w.data.clone(),
                firm_id: Uuid::new_v4(),
                template: crate::template::default_tree(),
            },
        );
        (segments, input)
    }

    #[test]
    fn split_twelve_pages_three_clients_and_undo() {
        let w = world();
        let original = print_run(&w.inbox);
        let root_before = snapshot(&w.root);
        let inbox_before = snapshot(&w.inbox);
        let (segments, input) = split_input(&w, &original, &[0, 1, 2]);
        let summary: Vec<_> = segments
            .iter()
            .map(|s| (s.pages.clone(), s.client_id.clone().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                (0..4, "1001".to_string()),
                (4..8, "1002".into()),
                (8..12, "1004".into())
            ],
            "Done-when: three pieces proposed"
        );
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.split_done, vec![original.clone()]);
        let pages: Vec<usize> = report
            .outcomes
            .iter()
            .map(|o| crate::pdf::page_count(o.result.as_ref().unwrap()).unwrap())
            .collect();
        assert_eq!(pages, [4, 4, 4]);
        assert!(!original.exists(), "the original left the inbox (X93)");
        let kept = w
            .data
            .join(crate::commit::SPLIT_ORIGINALS_DIR)
            .join(input.batch.to_string())
            .join(original.file_name().unwrap());
        assert!(kept.exists(), "a copy is kept for Undo");
        let left: Vec<String> = std::fs::read_dir(w.data.join(crate::pdf::TMP_DIR).join("s1"))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path().display().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !w.data.join(crate::pdf::TMP_DIR).join("s1").exists(),
            "temp folders removed: {left:?}"
        );
        let ops = oplog::read(&oplog::path(&w.data)).unwrap();
        assert_eq!(ops.records.iter().filter(|r| r.op == Op::Split).count(), 1);

        let lb = report.last_batch;
        assert_eq!(blocker(Some(&lb)), None);
        let undo = run_undo(&w.data, lb, false);
        assert!(
            undo.outcomes.iter().all(|o| o.1.is_ok()),
            "{:?}",
            undo.outcomes
        );
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(
            snapshot(&w.inbox),
            inbox_before,
            "Done-when: undo restores the original"
        );
        assert_eq!(
            snapshot(&w.root),
            root_before,
            "pieces and new folders gone"
        );
    }

    #[test]
    fn partial_split_keeps_the_original() {
        let w = world();
        let original = print_run(&w.inbox);
        let (_, all) = split_input(&w, &original, &[0, 1, 2]);
        let pieces = all.splits[0].pieces.clone();
        // First commit: pieces 1 and 3 only (piece 2 skipped).
        let mut first = all.clone();
        first.items.retain(|i| i.src != pieces[1]);
        let report = run_commit(&first, &AtomicUsize::new(0));
        assert_eq!(
            report.outcomes.iter().filter(|o| o.result.is_ok()).count(),
            2
        );
        assert!(report.split_done.is_empty());
        assert!(original.exists(), "X94: the original stays");
        assert!(
            report.warnings.iter().any(|w| w.contains("2 of 3 pieces")),
            "{:?}",
            report.warnings
        );
        // Marked as partly filed (survives restarts; Preview shows it as an error row).
        let marked = crate::commit::load_partial(&w.data);
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].original, original);

        // Undo deletes the two pieces, leaves the original and clears the mark.
        let undo = run_undo(&w.data, report.last_batch, false);
        assert!(
            undo.outcomes.iter().all(|o| o.1.is_ok()),
            "{:?}",
            undo.outcomes
        );
        assert!(original.exists());
        assert!(crate::commit::load_partial(&w.data).is_empty());

        // Later, only the remaining piece is filed: the original must stay in the inbox (the
        // pages of the undone pieces are only there), and it is marked again.
        let mut later = all.clone();
        later.batch = uuid::Uuid::new_v4();
        later.items.retain(|i| i.src == pieces[1]);
        later.splits[0].pieces = vec![pieces[1].clone()];
        let report = run_commit(&later, &AtomicUsize::new(0));
        assert!(
            report.outcomes.iter().all(|o| o.result.is_ok()),
            "{:?}",
            report.outcomes
        );
        assert!(report.split_done.is_empty());
        assert!(
            original.exists(),
            "not moved: not all pieces were filed together"
        );
        assert_eq!(crate::commit::load_partial(&w.data).len(), 1);
    }

    #[test]
    fn locked_piece_keeps_the_original_parked_on_undo() {
        use std::os::windows::fs::OpenOptionsExt;
        let w = world();
        let original = print_run(&w.inbox);
        let (_, input) = split_input(&w, &original, &[0, 1, 2]);
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert_eq!(report.split_done, vec![original.clone()]);
        let lb = report.last_batch;
        let kept = lb.split_originals[0].kept.clone();
        // Piece 2 is open in a viewer: it can't be deleted.
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&lb.items[1].dst)
            .unwrap();
        let undo = run_undo(&w.data, lb.clone(), false);
        drop(hold);
        assert!(undo.outcomes.iter().any(|o| o.1.is_err()));
        assert!(
            !original.exists(),
            "not restored while a piece is still filed"
        );
        assert!(kept.exists(), "still parked in data/split-originals");
        assert!(
            undo.warnings
                .iter()
                .any(|w| w.contains("could not be removed"))
        );
        assert!(lb.items[1].dst.exists());
    }

    #[test]
    fn changed_split_original_blocks_its_pieces() {
        let w = world();
        let original = print_run(&w.inbox);
        let (_, input) = split_input(&w, &original, &[0, 1, 2]);
        std::fs::write(&original, b"rescanned").unwrap();
        let report = run_commit(&input, &AtomicUsize::new(0));
        assert!(report.outcomes.iter().all(|o| {
            o.result
                .as_ref()
                .is_err_and(|e| e.contains("since it was split"))
        }));
        assert!(original.exists());
    }
}
