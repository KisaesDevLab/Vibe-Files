//! vibe-move (PA-35, Q-69): splits an over-large client tree. The old folder becomes the
//! archive, the new folder the active cabinet. Three steps, each previewed and confirmed:
//!
//! 1. **Permanent**: each client's top-level folder with the name given (any letter case)
//!    moves whole.
//! 2. **Date**: files modified on or after a date.
//! 3. **Year**: files with a four-digit year (1990 to next year) at or after the year given,
//!    in the file name or in a folder inside the client folder; the newest year counts.
//!
//! Files keep their path under the new folder. Moves go through Vibe Filer Desktop's engine
//! (`transfer`: a rename on one drive; across drives a copy, SHA-256 check, then the original
//! removed). A name already taken keeps both (` (2)`). Every move is written to a CSV log;
//! folders the move emptied are removed. Windows clutter and `_Vibe` folders never move.
//! [`undo`] puts a run back from its log (owner 2026-11-05), leaving any file changed since.
//! No catalog: this works on plain folders, before Vibe Files imports the new one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;

use chrono::{Datelike, Local, NaiveDate, TimeZone};

/// Destination paths longer than this are refused (Filer's X104).
pub const MAX_PATH: usize = filer_intake::template::MAX_PATH_LEN;
/// Years in names count from this one.
pub const FIRST_YEAR: i32 = 1990;
/// The default name of the permanent folder.
pub const PERMANENT: &str = "Permanent";

/// A top-level folder of the old tree (a client), or the loose files directly in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Client {
    /// Folder name; empty for the files directly in the old folder.
    pub name: String,
    pub files: usize,
    pub bytes: u64,
}

/// One step's rule.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// The client's top-level folder with this name (any letter case) moves whole.
    Permanent(String),
    /// Files modified on or after this date (local time).
    ModifiedSince(NaiveDate),
    /// Files with this year or a later one in their name or a folder inside the client folder.
    YearFrom(i32),
}

impl Rule {
    pub fn label(&self) -> String {
        match self {
            Rule::Permanent(n) => format!("permanent folder \"{n}\""),
            Rule::ModifiedSince(d) => format!("modified on or after {d}"),
            Rule::YearFrom(y) => format!("year {y} or later"),
        }
    }
}

/// A file to move.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Relative to the old folder, with `\`.
    pub rel: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Why it moves (for the preview and the log).
    pub why: String,
}

/// What a step would move.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub items: Vec<Item>,
    /// Windows clutter left in place.
    pub clutter: usize,
    /// Folders that could not be read.
    pub unreadable: Vec<String>,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }
}

/// Files never moved: Explorer's and Office's own, temporary files, partial copies.
pub fn is_clutter(name: &str) -> bool {
    let l = name.to_lowercase();
    l == "thumbs.db"
        || l == "desktop.ini"
        || l == ".ds_store"
        || l.starts_with("~$")
        || l.starts_with("~vfd-partial-")
        || l.ends_with(".tmp")
}

/// Folders never entered.
fn is_skipped_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case("_Vibe")
}

/// Checks the two folders: both given, different, neither inside the other.
pub fn check_folders(old: &Path, new: &Path) -> Result<(), String> {
    if old.as_os_str().is_empty() || new.as_os_str().is_empty() {
        return Err("Choose the old folder and the new folder.".into());
    }
    if !old.is_dir() {
        return Err(format!("{} is not a folder.", old.display()));
    }
    // Absolute and in lower case (Windows names ignore case); the new folder may not exist.
    let norm = |p: &Path| {
        let a = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        PathBuf::from(
            a.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_lowercase(),
        )
    };
    let (o, n) = (norm(old), norm(new));
    if o == n {
        return Err("The old and the new folder are the same.".into());
    }
    if n.starts_with(&o) {
        return Err("The new folder cannot be inside the old one.".into());
    }
    if o.starts_with(&n) {
        return Err("The old folder cannot be inside the new one.".into());
    }
    Ok(())
}

/// Every file under `dir` (clutter counted, not listed; `_Vibe` folders skipped), relative to
/// `base`.
fn walk(dir: &Path, base: &Path, out: &mut Vec<(PathBuf, std::fs::Metadata)>, plan: &mut Plan) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            plan.unreadable.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if !is_skipped_dir(&name) {
                walk(&e.path(), base, out, plan);
            }
        } else if is_clutter(&name) {
            plan.clutter += 1;
        } else if let Ok(m) = e.metadata() {
            let rel = e
                .path()
                .strip_prefix(base)
                .unwrap_or(&e.path())
                .to_path_buf();
            out.push((rel, m));
        }
    }
}

/// The old folder's clients (top-level folders, by name) and, first when there are any, the
/// loose files directly in it (name ""), with their file counts and sizes.
pub fn scan(old: &Path, stop: &AtomicBool, done: &AtomicUsize) -> Result<Vec<Client>, String> {
    let rd = std::fs::read_dir(old).map_err(|e| format!("{}: {e}", old.display()))?;
    let mut dirs = vec![];
    let mut loose = Client {
        name: String::new(),
        files: 0,
        bytes: 0,
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && !is_skipped_dir(&name) {
            dirs.push(name);
        } else if ft.is_file() && !is_clutter(&name) {
            loose.files += 1;
            loose.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    dirs.sort_by_key(|n| n.to_lowercase());
    let mut out = vec![];
    if loose.files > 0 {
        out.push(loose);
    }
    for name in dirs {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        let mut files = vec![];
        let mut p = Plan::default();
        walk(&old.join(&name), old, &mut files, &mut p);
        out.push(Client {
            files: files.len(),
            bytes: files.iter().map(|f| f.1.len()).sum(),
            name,
        });
        done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(out)
}

/// Standalone four-digit years in `s` from [`FIRST_YEAR`] to next year: not part of a longer
/// number (`120231` has none; `1099-2023` has 2023).
pub fn years_in(s: &str, next_year: i32) -> Vec<i32> {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 4
                && let Ok(y) = s[start..i].parse::<i32>()
                && (FIRST_YEAR..=next_year).contains(&y)
            {
                out.push(y);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The newest year in a file's name or the folders inside its client folder (`rel` is
/// relative to the old folder: its first part is the client folder, whose own name does not
/// count; a loose file has none).
pub fn newest_year(rel: &Path, next_year: i32) -> Option<(i32, &'static str)> {
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let (file, dirs) = parts.split_last()?;
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let in_name = years_in(&stem, next_year).into_iter().max();
    let inner = if dirs.is_empty() { &[][..] } else { &dirs[1..] };
    let in_dirs = inner.iter().flat_map(|d| years_in(d, next_year)).max();
    match (in_name, in_dirs) {
        (Some(a), Some(b)) if b > a => Some((b, "folder")),
        (Some(a), _) => Some((a, "name")),
        (None, Some(b)) => Some((b, "folder")),
        (None, None) => None,
    }
}

/// What `rule` moves from the ticked `clients` of `old` (names; "" for the loose files).
pub fn plan(old: &Path, clients: &[String], rule: &Rule) -> Plan {
    let mut p = Plan::default();
    let next_year = Local::now().year() + 1;
    let since = match rule {
        Rule::ModifiedSince(d) => d
            .and_hms_opt(0, 0, 0)
            .and_then(|t| Local.from_local_datetime(&t).earliest())
            .map(SystemTime::from),
        _ => None,
    };
    for c in clients {
        let mut files = vec![];
        match rule {
            Rule::Permanent(name) => {
                if c.is_empty() {
                    continue;
                }
                let Ok(rd) = std::fs::read_dir(old.join(c)) else {
                    p.unreadable.push(format!("{}", old.join(c).display()));
                    continue;
                };
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    // `_Vibe` never moves, even when typed as the name.
                    if e.file_type().is_ok_and(|t| t.is_dir())
                        && n.eq_ignore_ascii_case(name.trim())
                        && !is_skipped_dir(&n)
                    {
                        walk(&e.path(), old, &mut files, &mut p);
                    }
                }
            }
            _ if c.is_empty() => {
                // The loose files directly in the old folder.
                if let Ok(rd) = std::fs::read_dir(old) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().into_owned();
                        if !e.file_type().is_ok_and(|t| t.is_file()) {
                            continue;
                        }
                        if is_clutter(&n) {
                            p.clutter += 1;
                        } else if let Ok(m) = e.metadata() {
                            files.push((PathBuf::from(&n), m));
                        }
                    }
                }
            }
            _ => walk(&old.join(c), old, &mut files, &mut p),
        }
        for (rel, m) in files {
            let modified = m.modified().ok();
            let why = match rule {
                Rule::Permanent(_) => Some(rule.label()),
                Rule::ModifiedSince(_) => match (modified, since) {
                    (Some(t), Some(s)) if t >= s => {
                        let d: chrono::DateTime<Local> = t.into();
                        Some(format!("modified {}", d.format("%Y-%m-%d")))
                    }
                    _ => None,
                },
                Rule::YearFrom(y) => match newest_year(&rel, next_year) {
                    Some((found, wher)) if found >= *y => Some(format!("{found} in the {wher}")),
                    _ => None,
                },
            };
            if let Some(why) = why {
                p.items.push(Item {
                    rel,
                    size: m.len(),
                    modified,
                    why,
                });
            }
        }
    }
    p
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub moved: usize,
    pub kept_both: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub bytes: u64,
    /// Folders removed because the move emptied them.
    pub folders_removed: usize,
    pub log: Option<PathBuf>,
    pub stopped: bool,
    /// The log could not be written: the run stopped there (the moves logged are all there
    /// were).
    pub log_error: Option<String>,
}

/// Run options.
#[derive(Debug, Clone)]
pub struct Options {
    /// Copy, check and delete even on one drive (tests).
    pub force_copy: bool,
    /// Where the CSV log goes; default beside the new folder.
    pub log_dir: Option<PathBuf>,
    /// A file open in another program is tried again this many times (Q-72)…
    pub retries: u32,
    /// …this long apart.
    pub retry_wait: std::time::Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            force_copy: false,
            log_dir: None,
            retries: 3,
            retry_wait: std::time::Duration::from_secs(3),
        }
    }
}

/// Whether a failed move is a file open in another program (Windows sharing or lock
/// violation), worth trying again.
fn in_use(e: &str) -> bool {
    e.contains("os error 32") || e.contains("os error 33")
}

/// A CSV field.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Opens a new log file named `<name>-<kind>-<time>.csv` in the first of `dirs` that can be
/// written (a second log in the same second gets `-2`, `-3`, … rather than going elsewhere).
fn open_log(dirs: &[PathBuf], name: &str, kind: &str) -> Option<(PathBuf, std::fs::File)> {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    for dir in dirs {
        if std::fs::create_dir_all(dir).is_err() {
            continue;
        }
        for n in 1..100 {
            let suffix = if n == 1 {
                String::new()
            } else {
                format!("-{n}")
            };
            let path = dir.join(format!("{name}-{kind}-{stamp}{suffix}.csv"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => return Some((path, f)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
    }
    None
}

/// The log's columns (the undo reads them by name).
const LOG_HEADER: &str =
    "old path,new path,path in folder,size,modified,modified (seconds),step,reason,result";

/// A file's last-modified time, seconds since 1970.
fn secs(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A path's length as Windows counts it (UTF-16 units), as Filer's own check does.
fn path_len(p: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().count()
}

/// A file as moved: where it went, and its size and Date modified there (what the undo
/// later compares, so the log holds what was really moved).
struct Done {
    dst: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

/// Moves `src` to `want` (or a free name beside it): never replacing a file, trying again
/// while it is open in another program, keeping its Date modified across drives. `limit`:
/// refuse a destination over [`MAX_PATH`] (a move into the new folder; an undo puts a file
/// back where it already was). Folders made for it and left empty by a failure are removed.
fn move_one(
    src: &Path,
    want: &Path,
    limit: bool,
    opts: &Options,
    stop: &AtomicBool,
) -> Result<Done, String> {
    if !src.is_file() {
        return Err("no longer there".into());
    }
    let modified = std::fs::metadata(src).and_then(|m| m.modified()).ok();
    if limit && path_len(want) > MAX_PATH {
        return Err(format!(
            "the new path would be {} characters long (limit {MAX_PATH})",
            path_len(want)
        ));
    }
    // The folders this file needs that are not there yet: taken away again if it fails.
    let made: Vec<PathBuf> = want
        .ancestors()
        .skip(1)
        .take_while(|d| !d.as_os_str().is_empty() && !d.exists())
        .map(Path::to_path_buf)
        .collect();
    let result = (|| -> Result<Done, String> {
        if let Some(parent) = want.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create the folder: {e}"))?;
        }
        let dst = filer_intake::commit::free_path(want);
        if limit && path_len(&dst) > MAX_PATH {
            return Err(format!(
                "the new path would be {} characters long (limit {MAX_PATH})",
                path_len(&dst)
            ));
        }
        // Open in another program (Excel, Acrobat): tried again a few times, then left.
        let mut tries = 0;
        let method = loop {
            match filer_intake::commit::transfer(src, &dst, opts.force_copy) {
                Ok((m, _, _)) => break m,
                Err(e) if in_use(&e) && tries < opts.retries && !stop.load(Ordering::Relaxed) => {
                    tries += 1;
                    std::thread::sleep(opts.retry_wait);
                }
                Err(e) if in_use(&e) => return Err(format!("open in another program ({e})")),
                Err(e) => return Err(e),
            }
        };
        // A copy between drives is a new file: it gets the original's Date modified back.
        if method == filer_intake::commit::Method::CopyVerifyDelete
            && let Some(t) = modified
            && let Ok(f) = std::fs::OpenOptions::new().write(true).open(&dst)
        {
            let _ = f.set_modified(t);
        }
        // What is there now (a failed date fix-up, a drive that rounds times): logged as is.
        let m = std::fs::metadata(&dst).ok();
        Ok(Done {
            size: m.as_ref().map_or(0, |m| m.len()),
            modified: m.and_then(|m| m.modified().ok()),
            dst,
        })
    })();
    if result.is_err() {
        // Deepest first; only folders made here, and only while empty.
        for d in &made {
            let empty = std::fs::read_dir(d).is_ok_and(|mut r| r.next().is_none());
            if !empty || std::fs::remove_dir(d).is_err() {
                break;
            }
        }
    }
    result
}

/// Removes the folders in `dirs` the run left empty, deepest first, going up while they are
/// empty but never to or above `root`.
fn remove_emptied(mut dirs: Vec<PathBuf>, root: &Path) -> usize {
    let mut n = 0;
    dirs.sort();
    dirs.dedup();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for dir in dirs {
        let mut d = dir;
        while d.starts_with(root) && d != root {
            let empty = std::fs::read_dir(&d).is_ok_and(|mut r| r.next().is_none());
            if !empty || std::fs::remove_dir(&d).is_err() {
                break;
            }
            n += 1;
            match d.parent() {
                Some(p) => d = p.to_path_buf(),
                None => break,
            }
        }
    }
    n
}

/// Writes one log row, made to disk at once: the undo depends on it. A row that cannot be
/// written stops the run.
fn log_row(log: &mut std::fs::File, fields: &[String]) -> Result<(), String> {
    use std::io::Write;
    let line = fields.iter().map(|f| csv(f)).collect::<Vec<_>>().join(",");
    writeln!(log, "{line}")
        .and_then(|()| log.sync_data())
        .map_err(|e| {
            format!("the log could not be written ({e}); stopped so every move stays on record")
        })
}

/// Moves `plan`'s files from `old` to the same paths under `new`. Counts finished files in
/// `progress`; `stop` ends the run after the current file. Never replaces a file; a failed
/// file is reported and the rest go on.
pub fn apply(
    old: &Path,
    new: &Path,
    plan: &Plan,
    rule: &Rule,
    opts: &Options,
    progress: &AtomicUsize,
    stop: &AtomicBool,
) -> Result<Report, String> {
    use std::io::Write;
    check_folders(old, new)?;
    std::fs::create_dir_all(new).map_err(|e| format!("{}: {e}", new.display()))?;
    let mut rep = Report::default();
    // Beside the new folder, else inside it (never somewhere unexpected).
    let mut dirs = vec![];
    if let Some(d) = &opts.log_dir {
        dirs.push(d.clone());
    }
    if let Some(parent) = new.parent()
        && !parent.as_os_str().is_empty()
    {
        dirs.push(parent.to_path_buf());
    }
    dirs.push(new.join("_Vibe").join("move-logs"));
    let name = new
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vibe-move".into());
    let (log_path, mut log) = open_log(&dirs, &name, "move-log")
        .ok_or("The move log could not be created; nothing was moved.")?;
    rep.log = Some(log_path);
    writeln!(log, "{LOG_HEADER}").map_err(|e| e.to_string())?;
    let mut emptied: Vec<PathBuf> = vec![];
    for item in &plan.items {
        if stop.load(Ordering::Relaxed) {
            rep.stopped = true;
            break;
        }
        let src = old.join(&item.rel);
        let want = new.join(&item.rel);
        let (new_path, size, modified, result) = match move_one(&src, &want, true, opts, stop) {
            Ok(done) => {
                rep.moved += 1;
                rep.bytes += done.size;
                if let Some(parent) = src.parent() {
                    emptied.push(parent.to_path_buf());
                }
                let kept_both = done.dst != want;
                if kept_both {
                    rep.kept_both += 1;
                }
                (
                    done.dst,
                    done.size,
                    done.modified,
                    if kept_both {
                        "moved; kept both (the name was taken)".to_string()
                    } else {
                        "moved".to_string()
                    },
                )
            }
            Err(e) => {
                rep.failed.push((item.rel.clone(), e.clone()));
                (
                    PathBuf::new(),
                    item.size,
                    item.modified,
                    format!("not moved: {e}"),
                )
            }
        };
        let shown = modified
            .map(|t| {
                let d: chrono::DateTime<Local> = t.into();
                d.format("%Y-%m-%d %H:%M").to_string()
            })
            .unwrap_or_default();
        let row = [
            src.to_string_lossy().into_owned(),
            new_path.to_string_lossy().into_owned(),
            item.rel.to_string_lossy().into_owned(),
            size.to_string(),
            shown,
            modified
                .map(secs)
                .map(|s| s.to_string())
                .unwrap_or_default(),
            rule.label(),
            item.why.clone(),
            result,
        ];
        progress.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = log_row(&mut log, &row) {
            rep.log_error = Some(e);
            rep.stopped = true;
            break;
        }
    }
    rep.folders_removed = remove_emptied(emptied, old);
    Ok(rep)
}

/// One line of a CSV file, split into fields (`"` quoting as [`csv`] writes it).
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if cur.is_empty() => quoted = true,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// A file a move log says was moved.
#[derive(Debug, Clone, PartialEq)]
pub struct Moved {
    pub old: PathBuf,
    pub new: PathBuf,
    /// Relative to the two folders.
    pub rel: PathBuf,
    pub size: u64,
    pub modified: Option<i64>,
}

/// The two folders a log's moves were between: each row's paths less its place in them.
fn roots_of(m: &Moved) -> Option<(PathBuf, PathBuf)> {
    let depth = m.rel.components().count();
    let old = m.old.ancestors().nth(depth)?;
    // The new path may have a kept-both name: its folders are the rel's folders.
    let new = m.new.parent()?.ancestors().nth(depth.saturating_sub(1))?;
    // The rows must really be those paths under those folders.
    if !m.old.ends_with(&m.rel) {
        return None;
    }
    if let Some(dir) = m.rel.parent()
        && !m.new.parent()?.ends_with(dir)
    {
        return None;
    }
    Some((old.to_path_buf(), new.to_path_buf()))
}

/// The files `log` (a move log) says were moved, in its order. Refused unless every row is a
/// move between the same two folders (a hand-edited or other CSV cannot move files about).
pub fn read_log(log: &Path) -> Result<Vec<Moved>, String> {
    let text = std::fs::read_to_string(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut lines = text.lines();
    let head = csv_fields(lines.next().unwrap_or_default());
    let col = |name: &str| head.iter().position(|h| h == name);
    let (Some(o), Some(n), Some(r), Some(z), Some(res)) = (
        col("old path"),
        col("new path"),
        col("path in folder"),
        col("size"),
        col("result"),
    ) else {
        return Err(format!(
            "{} is not a vibe-move log this version can undo.",
            log.display()
        ));
    };
    let m = col("modified (seconds)");
    let mut out = vec![];
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let f = csv_fields(line);
        let get = |i: usize| f.get(i).map(String::as_str).unwrap_or_default();
        if !get(res).starts_with("moved") {
            continue;
        }
        out.push(Moved {
            old: PathBuf::from(get(o)),
            new: PathBuf::from(get(n)),
            rel: PathBuf::from(get(r)),
            size: get(z).parse().unwrap_or(0),
            modified: m.and_then(|i| get(i).parse().ok()),
        });
    }
    let mut roots: Option<(PathBuf, PathBuf)> = None;
    for row in &out {
        let bad = || {
            format!(
                "{} has a row that is not a move between two folders: {}",
                log.display(),
                row.old.display()
            )
        };
        let r = roots_of(row).ok_or_else(bad)?;
        match &roots {
            None => {
                check_folders(&r.0, &r.1).map_err(|e| format!("{}: {e}", log.display()))?;
                roots = Some(r);
            }
            Some(first) if *first != r => {
                return Err(format!(
                    "{} lists moves between more than one pair of folders; it was not written by one vibe-move run.",
                    log.display()
                ));
            }
            Some(_) => {}
        }
    }
    Ok(out)
}

/// Puts back what the move logged in `log`: each moved file goes from its new path to its old
/// one (a free name beside it if that is taken), but only when it is unchanged since the move
/// (same size and Date modified). Changed or missing files are left and reported. Writes its
/// own log beside `log`; removes the folders it empties in the new folder.
pub fn undo(
    log: &Path,
    opts: &Options,
    progress: &AtomicUsize,
    stop: &AtomicBool,
) -> Result<Report, String> {
    use std::io::Write;
    let rows = read_log(log)?;
    let mut rep = Report::default();
    let stem = log
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vibe-move".into());
    // Beside the move log, else the new folder's own log folder.
    let mut dirs: Vec<PathBuf> = log.parent().map(Path::to_path_buf).into_iter().collect();
    if let Some((_, new_root)) = rows.first().and_then(roots_of) {
        dirs.push(new_root.join("_Vibe").join("move-logs"));
    }
    let (ulog_path, mut ulog) = open_log(&dirs, &stem, "undo")
        .ok_or("The undo log could not be created; nothing was put back.")?;
    rep.log = Some(ulog_path);
    writeln!(ulog, "{LOG_HEADER}").map_err(|e| e.to_string())?;
    let new_root = rows.first().and_then(roots_of).map(|r| r.1);
    let mut emptied: Vec<PathBuf> = vec![];
    // Newest first, so a file moved twice (keep both) comes back in order.
    for row in rows.iter().rev() {
        if stop.load(Ordering::Relaxed) {
            rep.stopped = true;
            break;
        }
        let check = || -> Result<(), String> {
            let m = std::fs::metadata(&row.new).map_err(|_| "no longer in the new folder")?;
            if m.len() != row.size {
                return Err("changed since the move (size)".into());
            }
            if let (Some(want), Ok(t)) = (row.modified, m.modified())
                && secs(t) != want
            {
                return Err("changed since the move (Date modified)".into());
            }
            Ok(())
        };
        let (back, result) =
            match check().and_then(|()| move_one(&row.new, &row.old, false, opts, stop)) {
                Ok(done) => {
                    rep.moved += 1;
                    rep.bytes += row.size;
                    if let Some(parent) = row.new.parent() {
                        emptied.push(parent.to_path_buf());
                    }
                    if done.dst == row.old {
                        (done.dst, "put back".to_string())
                    } else {
                        rep.kept_both += 1;
                        (
                            done.dst,
                            "put back; kept both (the old name was taken)".to_string(),
                        )
                    }
                }
                Err(e) => {
                    rep.failed.push((row.rel.clone(), e.clone()));
                    (PathBuf::new(), format!("not put back: {e}"))
                }
            };
        let fields = [
            row.new.to_string_lossy().into_owned(),
            back.to_string_lossy().into_owned(),
            row.rel.to_string_lossy().into_owned(),
            row.size.to_string(),
            String::new(),
            row.modified.map(|s| s.to_string()).unwrap_or_default(),
            "undo".to_string(),
            String::new(),
            result,
        ];
        progress.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = log_row(&mut ulog, &fields) {
            rep.log_error = Some(e);
            rep.stopped = true;
            break;
        }
    }
    if let Some(root) = new_root {
        rep.folders_removed = remove_emptied(emptied, &root);
    }
    Ok(rep)
}

/// A date typed as `2024-01-31`, `1/31/2024` or `01/31/24`.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(s, "%m/%d/%Y"))
        .or_else(|_| NaiveDate::parse_from_str(s, "%m/%d/%y"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn set_modified(p: &Path, y: i32, m: u32, d: u32) {
        let t = Local.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(SystemTime::from(t)).unwrap();
    }

    fn rels(p: &Plan) -> Vec<String> {
        let mut v: Vec<String> = p
            .items
            .iter()
            .map(|i| i.rel.to_string_lossy().replace('\\', "/"))
            .collect();
        v.sort();
        v
    }

    /// A tree with invented clients: a Permanent folder, year folders, years in names.
    fn tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Old");
        let new = t.path().join("New");
        let a = old.join("Sample, Pat");
        put(&a.join("permanent/EIN letter.pdf"), "ein");
        put(&a.join("permanent/Setup/Articles.pdf"), "art");
        put(
            &a.join("Permanent Copies/x.pdf"),
            "not the permanent folder",
        );
        put(&a.join("Taxes/2019/W2 2024.pdf"), "two years");
        put(&a.join("Taxes/2019/W2.pdf"), "2019 only");
        put(&a.join("Taxes/2023/1099-2023.pdf"), "1099");
        put(&a.join("Taxes/Acct 120231.pdf"), "no year");
        put(&a.join("Taxes/Thumbs.db"), "clutter");
        put(&a.join("Taxes/~$draft.docx"), "clutter");
        put(&a.join("_Vibe/client.json"), "{}");
        let b = old.join("2025 Holdings LLC");
        put(&b.join("Notes.txt"), "client name year does not count");
        put(&b.join("Permanent/Bylaws.pdf"), "bylaws");
        put(&old.join("loose 2024.pdf"), "loose");
        (t, old, new)
    }

    #[test]
    fn years_are_standalone_four_digits_in_range() {
        assert_eq!(years_in("W2 2023", 2027), [2023]);
        assert_eq!(years_in("1099-2023", 2027), [2023]);
        assert_eq!(years_in("2023-04 Bank", 2027), [2023]);
        assert!(years_in("Acct 120231", 2027).is_empty());
        assert!(years_in("1099-MISC", 2027).is_empty());
        assert!(years_in("Form 1985", 2027).is_empty());
        assert!(years_in("2031 plan", 2027).is_empty());
        assert_eq!(
            newest_year(Path::new("Client/Taxes/2019/W2 2024.pdf"), 2027),
            Some((2024, "name"))
        );
        assert_eq!(
            newest_year(Path::new("Client/Taxes/2023/W2.pdf"), 2027),
            Some((2023, "folder"))
        );
        // The client folder's own name does not count.
        assert_eq!(
            newest_year(Path::new("2025 Holdings/Notes.txt"), 2027),
            None
        );
        assert_eq!(
            parse_date("2024-01-31"),
            NaiveDate::from_ymd_opt(2024, 1, 31)
        );
        assert_eq!(
            parse_date("1/31/2024"),
            NaiveDate::from_ymd_opt(2024, 1, 31)
        );
        assert_eq!(parse_date("nope"), None);
    }

    #[test]
    fn the_three_rules_pick_their_files() {
        let (_t, old, _new) = tree();
        let stop = AtomicBool::new(false);
        let done = AtomicUsize::new(0);
        let clients = scan(&old, &stop, &done).unwrap();
        let names: Vec<&str> = clients.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["", "2025 Holdings LLC", "Sample, Pat"]);
        let all: Vec<String> = clients.iter().map(|c| c.name.clone()).collect();

        let p = plan(&old, &all, &Rule::Permanent("PERMANENT".into()));
        assert_eq!(
            rels(&p),
            [
                "2025 Holdings LLC/Permanent/Bylaws.pdf",
                "Sample, Pat/permanent/EIN letter.pdf",
                "Sample, Pat/permanent/Setup/Articles.pdf"
            ]
        );

        let p = plan(&old, &all, &Rule::YearFrom(2023));
        assert_eq!(
            rels(&p),
            [
                "Sample, Pat/Taxes/2019/W2 2024.pdf",
                "Sample, Pat/Taxes/2023/1099-2023.pdf",
                "loose 2024.pdf"
            ]
        );
        assert_eq!(p.clutter, 2, "Thumbs.db and ~$ left");

        let f = old.join("Sample, Pat/Taxes/2019/W2.pdf");
        set_modified(&f, 2026, 3, 2);
        for other in ["Sample, Pat/Taxes/2023/1099-2023.pdf", "loose 2024.pdf"] {
            set_modified(&old.join(other), 2020, 1, 1);
        }
        let only_pat = vec!["Sample, Pat".to_string()];
        let p = plan(
            &old,
            &only_pat,
            &Rule::ModifiedSince(NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()),
        );
        assert!(rels(&p).contains(&"Sample, Pat/Taxes/2019/W2.pdf".to_string()));
        assert!(!rels(&p).iter().any(|r| r.contains("1099-2023")));
        assert!(!rels(&p).iter().any(|r| r.starts_with("2025")), "unticked");
    }

    fn run(old: &Path, new: &Path, rule: &Rule, force_copy: bool) -> Report {
        let all: Vec<String> = scan(old, &AtomicBool::new(false), &AtomicUsize::new(0))
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        let p = plan(old, &all, rule);
        apply(
            old,
            new,
            &p,
            rule,
            &Options {
                force_copy,
                ..Default::default()
            },
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn moves_keep_paths_keep_both_log_and_remove_emptied_folders() {
        let (t, old, new) = tree();
        // A name already taken in the new folder.
        put(
            &new.join("Sample, Pat/permanent/EIN letter.pdf"),
            "already there",
        );
        let rep = run(&old, &new, &Rule::Permanent("Permanent".into()), false);
        assert_eq!(rep.moved, 3, "{rep:?}");
        assert_eq!(rep.kept_both, 1);
        assert!(rep.failed.is_empty(), "{rep:?}");
        assert_eq!(
            std::fs::read_to_string(new.join("Sample, Pat/permanent/EIN letter (2).pdf")).unwrap(),
            "ein"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("Sample, Pat/permanent/EIN letter.pdf")).unwrap(),
            "already there",
            "never replaced"
        );
        assert!(new.join("2025 Holdings LLC/Permanent/Bylaws.pdf").is_file());
        // The emptied folders went; the client folders stay (other files).
        assert!(!old.join("Sample, Pat/permanent").exists());
        assert!(!old.join("2025 Holdings LLC/Permanent").exists());
        assert!(old.join("Sample, Pat/Taxes").is_dir());
        assert!(rep.folders_removed >= 3, "{rep:?}");
        // The log, beside the new folder.
        let log = rep.log.unwrap();
        assert_eq!(log.parent().unwrap(), t.path());
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.contains("kept both"));
    }

    #[test]
    fn copies_across_drives_are_verified() {
        let (_t, old, new) = tree();
        let rep = run(&old, &new, &Rule::YearFrom(2023), true);
        assert_eq!(rep.moved, 3, "{rep:?}");
        assert_eq!(
            std::fs::read_to_string(new.join("Sample, Pat/Taxes/2019/W2 2024.pdf")).unwrap(),
            "two years"
        );
        assert!(!old.join("Sample, Pat/Taxes/2019/W2 2024.pdf").exists());
        assert!(
            old.join("Sample, Pat/Taxes/2019/W2.pdf").exists(),
            "2019 stays"
        );
        assert!(new.join("loose 2024.pdf").is_file());
        assert!(
            old.join("Sample, Pat/_Vibe/client.json").is_file(),
            "_Vibe stays"
        );
    }

    /// Q-72: a file open in another program is tried again, then moved once it is closed;
    /// one that stays open is left and reported.
    #[test]
    fn a_file_open_elsewhere_is_retried() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_t, old, new) = tree();
        let file = old.join("Sample, Pat/Taxes/2023/1099-2023.pdf");
        let lock = |p: &Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(p)
                .unwrap()
        };
        let all = vec!["Sample, Pat".to_string()];
        let rule = Rule::YearFrom(2023);
        let run = |retries: u32| {
            let p = plan(&old, &all, &rule);
            apply(
                &old,
                &new,
                &p,
                &rule,
                &Options {
                    retries,
                    retry_wait: std::time::Duration::from_millis(300),
                    ..Default::default()
                },
                &AtomicUsize::new(0),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        // Held throughout: left, and said why.
        let held = lock(&file);
        let rep = run(1);
        assert_eq!(rep.failed.len(), 1, "{rep:?}");
        assert!(
            rep.failed[0].1.contains("open in another program"),
            "{rep:?}"
        );
        assert!(file.is_file());
        drop(held);
        // Closed while it waits: moved.
        let held = lock(&file);
        let closer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            drop(held);
        });
        let rep = run(5);
        closer.join().unwrap();
        assert!(rep.failed.is_empty(), "{rep:?}");
        assert!(new.join("Sample, Pat/Taxes/2023/1099-2023.pdf").is_file());
    }

    /// A move is undone from its log: files back where they were (paths with commas, keep
    /// both), Date modified kept across drives, a file changed since left; emptied folders
    /// in the new folder removed.
    #[test]
    fn undo_puts_files_back_from_the_log() {
        let (_t, old, new) = tree();
        let before_t = std::fs::metadata(old.join("Sample, Pat/Taxes/2019/W2 2024.pdf"))
            .unwrap()
            .modified()
            .unwrap();
        set_modified(&old.join("Sample, Pat/Taxes/2019/W2 2024.pdf"), 2021, 6, 1);
        let rep = run(&old, &new, &Rule::YearFrom(2023), true);
        assert_eq!(rep.moved, 3, "{rep:?}");
        let moved = new.join("Sample, Pat/Taxes/2019/W2 2024.pdf");
        let t = std::fs::metadata(&moved).unwrap().modified().unwrap();
        assert_ne!(t, before_t);
        assert_eq!(
            chrono::DateTime::<Local>::from(t)
                .format("%Y-%m-%d")
                .to_string(),
            "2021-06-01",
            "Date modified kept across drives"
        );
        // One file changed after the move: it stays.
        std::fs::write(new.join("loose 2024.pdf"), "edited after the move").unwrap();
        let log = rep.log.unwrap();
        assert_eq!(read_log(&log).unwrap().len(), 3);
        let back = undo(
            &log,
            &Options {
                force_copy: true,
                ..Default::default()
            },
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(back.moved, 2, "{back:?}");
        assert_eq!(back.failed.len(), 1);
        assert!(
            back.failed[0].1.contains("changed since the move"),
            "{back:?}"
        );
        assert_eq!(
            std::fs::read_to_string(old.join("Sample, Pat/Taxes/2019/W2 2024.pdf")).unwrap(),
            "two years"
        );
        assert!(old.join("Sample, Pat/Taxes/2023/1099-2023.pdf").is_file());
        assert!(!new.join("Sample, Pat").exists(), "emptied folders removed");
        assert!(new.join("loose 2024.pdf").is_file(), "the edited one stays");
        assert!(back.log.unwrap().to_string_lossy().contains("-undo-"));
    }

    /// Review fixes: a path over 240 characters in the old tree comes back on undo; a log
    /// whose rows are not moves between one pair of folders is refused; a file that fails
    /// leaves no empty folders in the new folder; `_Vibe` never moves even when typed.
    #[test]
    fn undo_long_paths_tampered_logs_and_failures() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Old");
        let new = t.path().join("N");
        // An old path well over 240 characters (the reason to split the tree).
        let deep = "Ü".repeat(60);
        let rel = format!("Client/{deep}/{deep}/{deep}/2024 return.pdf");
        let long = old.join(&rel);
        put(&long, "deep");
        assert!(path_len(&long) > MAX_PATH);
        let opts = Options::default();
        let p = plan(&old, &["Client".into()], &Rule::YearFrom(2024));
        assert_eq!(p.items.len(), 1);
        let rep = apply(
            &old,
            &new,
            &p,
            &Rule::YearFrom(2024),
            &opts,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        // The new path is long too: refused, and no empty folders left behind in the new one.
        assert_eq!(rep.failed.len(), 1, "{rep:?}");
        assert!(rep.failed[0].1.contains("characters long"), "{rep:?}");
        assert!(
            !new.join("Client").exists(),
            "no empty folders left in the new folder"
        );
        assert!(long.is_file());

        // A shallower copy whose old path is long but whose new path fits.
        let old2 = t.path().join(format!("Old {}", "x".repeat(120)));
        let rel2 = format!("Client/{}/2024.pdf", "y".repeat(100));
        put(&old2.join(&rel2), "long old");
        let new2 = t.path().join("M");
        assert!(path_len(&old2.join(&rel2)) > MAX_PATH);
        let p = plan(&old2, &["Client".into()], &Rule::YearFrom(2024));
        let rep = apply(
            &old2,
            &new2,
            &p,
            &Rule::YearFrom(2024),
            &opts,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(rep.moved, 1, "{rep:?}");
        let log = rep.log.unwrap();
        let back = undo(&log, &opts, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap();
        assert_eq!(back.moved, 1, "a long old path comes back: {back:?}");
        assert!(old2.join(&rel2).is_file());

        // A log edited to move a file somewhere else is refused.
        let text = std::fs::read_to_string(&log).unwrap();
        let elsewhere = t.path().join("Elsewhere");
        let forged = log.with_file_name("forged.csv");
        std::fs::write(
            &forged,
            text.replace(
                &old2.to_string_lossy().into_owned(),
                &elsewhere.to_string_lossy(),
            ),
        )
        .unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        let row = lines[1].clone();
        lines.push(row.replace(
            &new2.to_string_lossy().into_owned(),
            &elsewhere.to_string_lossy(),
        ));
        std::fs::write(
            &forged,
            lines.join(
                "
",
            ),
        )
        .unwrap();
        assert!(read_log(&forged).is_err());

        // _Vibe typed as the permanent name: nothing.
        let (_t3, old3, _new3) = tree();
        let p = plan(
            &old3,
            &["Sample, Pat".into()],
            &Rule::Permanent("_vibe".into()),
        );
        assert!(p.items.is_empty());

        // A file held open: no folders left behind for it in the new folder.
        let (_t4, old4, new4) = tree();
        let held = old4.join("Sample, Pat/Taxes/2023/1099-2023.pdf");
        let _lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .unwrap();
        let p = plan(&old4, &["Sample, Pat".into()], &Rule::YearFrom(2023));
        let rep = apply(
            &old4,
            &new4,
            &p,
            &Rule::YearFrom(2023),
            &Options {
                retries: 0,
                ..Default::default()
            },
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(rep.failed.len(), 1);
        assert!(
            !new4.join("Sample, Pat/Taxes/2023").exists(),
            "no empty folder for it"
        );
        assert!(new4.join("Sample, Pat/Taxes/2019/W2 2024.pdf").is_file());
    }

    #[test]
    fn folders_are_checked() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Old");
        std::fs::create_dir_all(&old).unwrap();
        assert!(check_folders(&old, &old).is_err());
        assert!(check_folders(&old, &old.join("inside")).is_err());
        assert!(check_folders(&old.join("x"), &t.path().join("New")).is_err());
        assert!(check_folders(&old, &t.path().join("New")).is_ok());
    }
}
