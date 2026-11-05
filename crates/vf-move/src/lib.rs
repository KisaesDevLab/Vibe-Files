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
//! folders the move emptied are removed. Windows clutter and `_Vibe` folders never move. No
//! catalog: this works on plain folders, before Vibe Files imports the new one.

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
                    if e.file_type().is_ok_and(|t| t.is_dir())
                        && n.eq_ignore_ascii_case(name.trim())
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

/// How one file went.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Moved,
    /// Moved under another name: the new folder had that name already.
    KeptBoth(PathBuf),
    Failed(String),
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
}

/// Run options.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Copy, check and delete even on one drive (tests).
    pub force_copy: bool,
    /// Where the CSV log goes; default beside the new folder.
    pub log_dir: Option<PathBuf>,
}

/// A CSV field.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Where the log of a run goes: beside the new folder (`<new name>-move-log-<time>.csv`), or
/// inside it under `_Vibe\move-logs` when that place cannot be written.
fn open_log(new: &Path, opts: &Options) -> Option<(PathBuf, std::fs::File)> {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let name = new
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vibe-move".into());
    let mut tries = vec![];
    if let Some(d) = &opts.log_dir {
        tries.push(d.clone());
    }
    if let Some(parent) = new.parent()
        && !parent.as_os_str().is_empty()
    {
        tries.push(parent.to_path_buf());
    }
    tries.push(new.join("_Vibe").join("move-logs"));
    for dir in tries {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{name}-move-log-{stamp}.csv"));
        if let Ok(f) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            return Some((path, f));
        }
    }
    None
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
    let (log_path, mut log) =
        open_log(new, opts).ok_or("The move log could not be created; nothing was moved.")?;
    rep.log = Some(log_path);
    writeln!(log, "old path,new path,size,modified,step,reason,result")
        .map_err(|e| e.to_string())?;
    let mut emptied: Vec<PathBuf> = vec![];
    for item in &plan.items {
        if stop.load(Ordering::Relaxed) {
            rep.stopped = true;
            break;
        }
        let src = old.join(&item.rel);
        let want = new.join(&item.rel);
        let outcome = (|| -> Result<Outcome, String> {
            if !src.is_file() {
                return Err("no longer there".into());
            }
            if let Some(parent) = want.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create the folder: {e}"))?;
            }
            let dst = filer_intake::commit::free_path(&want);
            if dst.as_os_str().len() > MAX_PATH {
                return Err(format!(
                    "the new path would be {} characters long (limit {MAX_PATH})",
                    dst.as_os_str().len()
                ));
            }
            filer_intake::commit::transfer(&src, &dst, opts.force_copy)?;
            Ok(if dst == want {
                Outcome::Moved
            } else {
                Outcome::KeptBoth(dst)
            })
        })();
        let (new_path, result) = match &outcome {
            Ok(Outcome::Moved) => {
                rep.moved += 1;
                rep.bytes += item.size;
                (want.clone(), "moved".to_string())
            }
            Ok(Outcome::KeptBoth(p)) => {
                rep.moved += 1;
                rep.kept_both += 1;
                rep.bytes += item.size;
                (
                    p.clone(),
                    "moved; kept both (the name was taken)".to_string(),
                )
            }
            Ok(Outcome::Failed(e)) | Err(e) => {
                rep.failed.push((item.rel.clone(), e.clone()));
                (PathBuf::new(), format!("not moved: {e}"))
            }
        };
        if matches!(outcome, Ok(Outcome::Moved | Outcome::KeptBoth(_)))
            && let Some(parent) = src.parent()
        {
            emptied.push(parent.to_path_buf());
        }
        let modified = item
            .modified
            .map(|t| {
                let d: chrono::DateTime<Local> = t.into();
                d.format("%Y-%m-%d %H:%M").to_string()
            })
            .unwrap_or_default();
        let _ = writeln!(
            log,
            "{},{},{},{},{},{},{}",
            csv(&src.to_string_lossy()),
            csv(&new_path.to_string_lossy()),
            item.size,
            modified,
            csv(&rule.label()),
            csv(&item.why),
            csv(&result)
        );
        progress.fetch_add(1, Ordering::Relaxed);
    }
    let _ = log.flush();
    let _ = log.sync_all();
    // Folders the move emptied, deepest first, up to (not including) the old folder.
    emptied.sort();
    emptied.dedup();
    emptied.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for dir in emptied {
        let mut d = dir;
        while d.starts_with(old) && d != old {
            let empty = std::fs::read_dir(&d).is_ok_and(|mut r| r.next().is_none());
            if !empty || std::fs::remove_dir(&d).is_err() {
                break;
            }
            rep.folders_removed += 1;
            match d.parent() {
                Some(p) => d = p.to_path_buf(),
                None => break,
            }
        }
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
                log_dir: None,
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
