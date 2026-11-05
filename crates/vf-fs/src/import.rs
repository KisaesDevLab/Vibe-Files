//! Folder-tree import (PHASES P10): adopt an existing tree (a FileCenter cabinet, a legacy firm
//! tree) in place. Files are not moved.
//!
//! The cabinet is opened (or created) over the tree's root first, so the tree's top-level
//! folders are the candidate client folders (a FileCenter cabinet's drawers). [`plan`] proposes,
//! with a confidence for each:
//! - **client folders** with Filer's own root scan (`sentinel::scan_root`: the client ID in the
//!   folder name, or a close name), confirmed by a word-for-word name comparison
//!   (`vf_extract::matching::name_score`); an ID in the name with no client in the list proposes
//!   a new client;
//! - **tax years** from any year in a folder name (`2023 Tax`, `TY2022`), else the file name;
//! - **doc types** from keywords of the cabinet's doc types in the file name, else its folders.
//!
//! [`apply`] writes sentinels (and, if asked, renames client folders to the firm template) in
//! one Transaction, runs the rebuild for the catalog rows, sets each file's doc type and year in
//! one more (`docmeta`, so a rebuild keeps them); the caller then queues the OCR backlog. [`undo`] reverses
//! both. What is not sure enough is left for the person: [`Plan::remainder`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use filer_core::clients::{Client, ClientList};
use filer_core::template;
use serde_json::json;
use uuid::Uuid;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Transaction};

use crate::clients::{self, master_dir};
use crate::docmeta::MetaEdit;
use crate::rebuild::{self, files_under, rel};

/// Label of the bind (and rename) Transaction.
pub const LABEL: &str = "folder_import";
/// Label of the doc type / year Transaction.
pub const META_LABEL: &str = "folder_import_meta";
/// Proposals at or above this are applied without the person choosing them.
pub const AUTO: f32 = 0.85;

/// A client a folder is proposed for.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientPick {
    pub ext_id: String,
    pub uuid: Uuid,
    pub name: String,
    /// Not in the client list yet: added to it when applied.
    pub new: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    /// Folder name (cabinet-relative; top level).
    pub rel: String,
    pub files: usize,
    /// Already has a sentinel: nothing to bind.
    pub bound: bool,
    pub client: Option<ClientPick>,
    /// How the client was found, for the preview.
    pub how: String,
    pub confidence: f32,
    /// The firm template's name for the folder, when different from its name.
    pub template_name: Option<String>,
    /// Bound when applied (defaults to confidence ≥ [`AUTO`]; the person can change it).
    pub include: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub rel: String,
    /// Index into [`Plan::folders`]; `None` for files directly in the cabinet root.
    pub folder: Option<usize>,
    pub tax_year: Option<i32>,
    /// `folder`, `file name`, `permanent`, or empty when not found.
    pub year_from: &'static str,
    /// (doc type id, name).
    pub doc_type: Option<(String, String)>,
    pub type_confidence: f32,
    /// Why no doc type, or which word decided it.
    pub type_how: String,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub folders: Vec<FolderRow>,
    pub files: Vec<FileRow>,
    /// Every client in the list, for the person to choose from.
    pub clients: Vec<ClientPick>,
    /// Top-level folders left out (names starting `_`, `.` or `~`): listed, not imported.
    pub skipped: Vec<String>,
}

/// Why a file is in the remainder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    Client,
    DocType,
    Year,
}

impl Gap {
    pub fn label(self) -> &'static str {
        match self {
            Gap::Client => "no client",
            Gap::DocType => "no doc type",
            Gap::Year => "no tax year",
        }
    }
}

impl Plan {
    fn folder_ok(&self, f: &FileRow) -> bool {
        f.folder.is_some_and(|i| {
            let r = &self.folders[i];
            (r.include || r.bound) && r.client.is_some()
        })
    }

    /// What a file still lacks once the plan is applied (empty: fully mapped).
    pub fn gaps(&self, f: &FileRow) -> Vec<Gap> {
        let mut g = vec![];
        if !self.folder_ok(f) {
            g.push(Gap::Client);
        }
        if f.doc_type.is_none() || f.type_confidence < AUTO {
            g.push(Gap::DocType);
        }
        if f.year_from.is_empty() {
            g.push(Gap::Year);
        }
        g
    }

    /// Files mapped to a client, a doc type and a tax year (or permanent).
    pub fn auto_mapped(&self) -> usize {
        self.files
            .iter()
            .filter(|f| self.gaps(f).is_empty())
            .count()
    }

    /// Files left for manual handling, with what each lacks.
    pub fn remainder(&self) -> Vec<(&FileRow, Vec<Gap>)> {
        self.files
            .iter()
            .map(|f| (f, self.gaps(f)))
            .filter(|(_, g)| !g.is_empty())
            .collect()
    }

    /// Folders that will not be bound.
    pub fn unmapped_folders(&self) -> Vec<&FolderRow> {
        self.folders
            .iter()
            .filter(|r| !r.bound && !(r.include && r.client.is_some()))
            .collect()
    }

    /// The remainder as CSV: unmapped folders, then files.
    pub fn report_csv(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut out = String::from("kind,path,missing,proposed client,doc type,tax year\n");
        for f in &self.skipped {
            out.push_str(&format!(
                "folder,{},{},,,\n",
                q(f),
                q("skipped: the name starts with _ . or ~")
            ));
        }
        for r in self.unmapped_folders() {
            out.push_str(&format!(
                "folder,{},{},{},,\n",
                q(&r.rel),
                q(if r.client.is_some() {
                    "not chosen"
                } else {
                    "no client"
                }),
                q(&r.client
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_default())
            ));
        }
        for (f, gaps) in self.remainder() {
            let missing: Vec<&str> = gaps.iter().map(|g| g.label()).collect();
            let client = f
                .folder
                .and_then(|i| self.folders[i].client.as_ref())
                .map(|c| c.name.clone())
                .unwrap_or_default();
            out.push_str(&format!(
                "file,{},{},{},{},{}\n",
                q(&f.rel),
                q(&missing.join("; ")),
                q(&client),
                q(&f.doc_type.as_ref().map(|d| d.1.clone()).unwrap_or_default()),
                f.tax_year.map(|y| y.to_string()).unwrap_or_default()
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Years and doc types

/// A plausible tax year: a run of exactly four digits in 1990..=2100.
pub fn year_in(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 4 {
                let y: i32 = s[start..i].parse().unwrap_or(0);
                if (1990..=2100).contains(&y) {
                    return Some(y);
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Words people use for each seeded doc type in folder and file names. `!` marks a word that
/// only counts in a file name (a `Tax Returns` folder holds the return's source documents too).
const SYNONYMS: &[(&str, &[&str])] = &[
    ("W-2", &["W2", "W2C", "wage statement"]),
    ("1099-INT", &["1099INT", "interest income"]),
    ("1099-DIV", &["1099DIV", "dividends"]),
    ("1099-B", &["1099B", "proceeds"]),
    ("1099-R", &["1099R", "pension", "retirement distribution"]),
    ("1099-NEC", &["1099NEC", "nonemployee compensation"]),
    ("1099-MISC", &["1099MISC"]),
    ("1099-K", &["1099K"]),
    ("1099-G", &["1099G", "unemployment"]),
    (
        "1098",
        &["1098", "mortgage interest", "1098E", "1098T", "tuition"],
    ),
    (
        "1095",
        &["1095", "1095A", "1095B", "1095C", "health coverage"],
    ),
    ("K-1 (1065)", &["K1", "K1 1065", "partnership K1"]),
    ("K-1 (1120-S)", &["K1", "K1 1120S", "S corp K1"]),
    ("K-1 (1041)", &["K1", "K1 1041", "trust K1", "estate K1"]),
    ("SSA-1099", &["SSA1099", "social security"]),
    (
        "Brokerage statement",
        &["brokerage", "investment statement"],
    ),
    (
        "Property tax statement",
        &["property tax", "real estate tax"],
    ),
    (
        "Charitable receipt",
        &["charitable", "donation", "contributions", "charity"],
    ),
    ("Organizer", &["organizer", "questionnaire"]),
    (
        "8879 e-file authorization",
        &["8879", "efile authorization", "e file auth"],
    ),
    (
        "Return copy — federal",
        &[
            "!1040",
            "!1120",
            "!1120S",
            "!1065",
            "!1041",
            "!990",
            "!tax return",
            "!federal return",
            "!return copy",
            "!client copy",
            "!filing copy",
        ],
    ),
    ("Return copy — state", &["!state return", "!state copy"]),
    ("Extension", &["extension", "4868", "7004", "8868"]),
    (
        "Estimate vouchers",
        &["estimates", "estimated tax", "1040ES", "vouchers"],
    ),
    ("Engagement letter", &["engagement", "engagement letter"]),
    (
        "Notice / correspondence",
        &[
            "notice",
            "IRS letter",
            "correspondence",
            "letters",
            "CP2000",
        ],
    ),
    ("§7216 consent", &["7216", "consent"]),
    ("Workpaper", &["workpaper", "workpapers", "work papers"]),
    (
        "Depreciation schedule",
        &["depreciation", "4562", "fixed assets"],
    ),
    ("Trial balance", &["trial balance", "TB"]),
    (
        "Bank statement",
        &["bank statement", "bank stmt", "bank", "bank recs"],
    ),
    (
        "Financial statements",
        &[
            "financial statements",
            "financials",
            "balance sheet",
            "profit and loss",
            "P&L",
        ],
    ),
    (
        "Payroll report",
        &["payroll", "payroll report", "payroll register"],
    ),
    ("Payroll tax return", &["941", "940", "W3", "payroll tax"]),
    ("Sales tax return", &["sales tax", "sales and use"]),
    (
        "Formation documents",
        &[
            "articles",
            "formation",
            "incorporation",
            "bylaws",
            "certificate of organization",
        ],
    ),
    ("EIN letter", &["EIN", "SS4", "CP575", "EIN letter"]),
    ("S election (2553)", &["2553", "S election"]),
    (
        "Operating agreement",
        &["operating agreement", "partnership agreement"],
    ),
    ("POA (2848)", &["2848", "POA", "power of attorney"]),
    ("Tax information authorization (8821)", &["8821"]),
    ("Stock basis", &["stock basis", "basis"]),
    ("Invoice", &["invoice", "invoices", "bill"]),
];

/// Upper-case alphanumeric words of `s` (`&` kept as a word: `P&L`).
fn words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_uppercase());
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if ch == '&' {
                out.push("&".into());
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct Keyword {
    /// The keyword's words joined (`TRIALBALANCE`, `W2`, `1099INT`).
    compact: String,
    type_idx: usize,
    file_only: bool,
}

/// Doc types and the words that point at them.
struct Matcher {
    types: Vec<(String, String, bool)>, // id, name, permanent
    keywords: Vec<Keyword>,
}

impl Matcher {
    fn load(conn: &Connection) -> Result<Matcher, String> {
        let types: Vec<(String, String, bool)> = conn
            .prepare(
                "SELECT id, name, coalesce(default_folder_tpl, '') FROM doc_type
                 WHERE deleted_at IS NULL ORDER BY sort, name",
            )
            .and_then(|mut st| {
                st.query_map([], |r| {
                    let tpl: String = r.get(2)?;
                    Ok((r.get(0)?, r.get(1)?, !tpl.contains("{Year}")))
                })?
                .collect()
            })
            .map_err(|e| e.to_string())?;
        let mut keywords = vec![];
        let mut add = |k: &str, i: usize| {
            let (file_only, k) = match k.strip_prefix('!') {
                Some(k) => (true, k),
                None => (false, k),
            };
            let compact = words(k).concat();
            if !compact.is_empty() {
                keywords.push(Keyword {
                    compact,
                    type_idx: i,
                    file_only,
                });
            }
        };
        for (i, (_, name, _)) in types.iter().enumerate() {
            // The name itself, and its parts (`POA (2848)` → `POA`, `2848`).
            add(name, i);
            // A K-1's part is the return it comes from (`K-1 (1065)`): `1065` alone is the return.
            let k1 = name.starts_with("K-1");
            for part in name.split(['(', ')', '/', '—']) {
                if !k1 && part.trim().len() > 2 && !part.trim().eq_ignore_ascii_case("copy") {
                    add(part.trim(), i);
                }
            }
            if let Some((_, syn)) = SYNONYMS.iter().find(|(n, _)| n == name) {
                for k in *syn {
                    add(k, i);
                }
            }
        }
        Ok(Matcher { types, keywords })
    }

    /// The doc types `text` names, each with its best keyword's rank (form numbers first, then
    /// longer keywords).
    fn hits(&self, text: &str, in_file_name: bool) -> BTreeMap<usize, usize> {
        let w = words(text);
        let mut found: BTreeMap<usize, usize> = BTreeMap::new();
        for k in &self.keywords {
            if k.file_only && !in_file_name {
                continue;
            }
            let hit = (0..w.len()).any(|i| {
                let mut joined = String::new();
                for part in w.iter().skip(i).take(4) {
                    joined.push_str(part);
                    if joined == k.compact
                        || (joined.len() == k.compact.len() + 1
                            && joined.ends_with('S')
                            && joined.starts_with(&k.compact))
                    {
                        return true;
                    }
                    if joined.len() > k.compact.len() + 1 {
                        break;
                    }
                }
                false
            });
            if hit {
                // A form number (`1099DIV`, `8879`) says more than any word (`BROKERAGE`).
                let rank = k.compact.len()
                    + if k.compact.bytes().any(|b| b.is_ascii_digit()) {
                        100
                    } else {
                        0
                    };
                let e = found.entry(k.type_idx).or_default();
                *e = (*e).max(rank);
            }
        }
        found
    }

    /// The doc type for a file: its name first, then its folders from the deepest.
    fn decide(&self, file_stem: &str, folders: &[&str]) -> (Option<usize>, f32, String) {
        let pick = |hits: BTreeMap<usize, usize>| -> Option<Result<usize, Vec<usize>>> {
            let best = *hits.values().max()?;
            let top: Vec<usize> = hits
                .iter()
                .filter(|(_, l)| **l == best)
                .map(|(i, _)| *i)
                .collect();
            Some(if top.len() == 1 { Ok(top[0]) } else { Err(top) })
        };
        let names = |v: &[usize]| {
            v.iter()
                .map(|i| self.types[*i].1.as_str())
                .collect::<Vec<_>>()
                .join(" or ")
        };
        match pick(self.hits(file_stem, true)) {
            Some(Ok(i)) => return (Some(i), 0.95, "file name".into()),
            Some(Err(v)) => return (None, 0.5, format!("file name could be {}", names(&v))),
            None => {}
        }
        for f in folders.iter().rev() {
            match pick(self.hits(f, false)) {
                Some(Ok(i)) => return (Some(i), 0.88, format!("folder \"{f}\"")),
                Some(Err(v)) => {
                    return (None, 0.5, format!("folder \"{f}\" could be {}", names(&v)));
                }
                None => {}
            }
        }
        (None, 0.0, "no doc type word".into())
    }
}

// ---------------------------------------------------------------------------------------------
// Client folders

fn pick(c: &Client, new: bool) -> ClientPick {
    ClientPick {
        ext_id: c.id.clone(),
        uuid: c.uuid,
        name: c.name.clone(),
        new,
    }
}

/// The ID at the end of a folder name the way Filer reads it: `Name (ID)` or `Name_ID`.
fn id_in_name(id_pattern: &str, name: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(&format!(
        r"(?:\s*\((?P<p>{id_pattern})\)\s*$)|(?:_(?P<u>{id_pattern})$)"
    ))
    .ok()?;
    let c = re.captures(name)?;
    let id = c.name("p").or_else(|| c.name("u"))?.as_str().to_string();
    let start = c.get(0)?.start();
    Some((id, name[..start].trim().to_string()))
}

/// Whether a trailing token can be a client ID for a new client: it has a digit and is not a
/// year (`Tax Returns (2019)`, `Reyes (DECEASED)`, `Archive_2019` are not clients).
fn looks_like_client_id(id: &str) -> bool {
    id.chars().any(|c| c.is_ascii_digit())
        && !(id.len() == 4 && id.parse::<u32>().is_ok_and(|y| (1990..=2100).contains(&y)))
}

/// How sure a folder whose ID is a list client's is, by how close the names are (0–1): the
/// names agree (score 0.6 or more) → 90–100 %, ticked; they do not → 10–40 %, with no "how"
/// (the caller names the client).
fn id_match(name_score: f32) -> (Option<String>, f32) {
    let ns = name_score.clamp(0.0, 1.0);
    if ns >= 0.6 {
        (
            Some("client ID and name".into()),
            0.9 + (ns - 0.6) / 0.4 * 0.1,
        )
    } else {
        (None, 0.1 + ns / 0.6 * 0.3)
    }
}

/// How sure a folder is that only looks like a client's name (Filer's fuzzy score, from its
/// suggest threshold up to 1): 60–84 %, never ticked on its own, closer names higher.
fn fuzzy_match(score: f32) -> f32 {
    let s = score.clamp(0.0, 1.0);
    0.6 + s * s * 0.24
}

/// The client a top-level folder is for, with how and how sure.
fn folder_client(
    b: &filer_intake::sentinel::FolderBinding,
    list: &ClientList,
    id_pattern: &str,
) -> (Option<ClientPick>, String, f32) {
    use filer_intake::sentinel::Status;
    let name = b.name();
    match b.status {
        Status::UnboundNamed => {
            let c = b.client_id.as_deref().and_then(|id| list.match_by_id(id));
            let base = id_in_name(id_pattern, &name).map_or(name.clone(), |(_, rest)| rest);
            return match c {
                // The name must agree too: an old tree's IDs can be another client's now.
                Some(c) => {
                    let ns = vf_extract::matching::name_score(&base, &c.name);
                    let (how, conf) = id_match(ns);
                    let how =
                        how.unwrap_or_else(|| format!("the ID is {}'s, the name is not", c.name));
                    (Some(pick(c, false)), how, conf)
                }
                None => (None, String::new(), 0.0),
            };
        }
        Status::Orphan | Status::Unparseable | Status::Duplicate | Status::Ignored => {
            return (
                None,
                b.note
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", b.status).to_lowercase()),
                0.0,
            );
        }
        _ => {}
    }
    let (base, id) = match id_in_name(id_pattern, &name) {
        Some((id, rest)) => (rest, Some(id)),
        None => (name.clone(), None),
    };
    // Word-for-word: every word of the shorter name in the other (`Reyes, Dana` = `Dana Reyes`);
    // among those, the clients sharing the most meaningful words (`John Smith` is `Smith, John`,
    // not the client `Smith`).
    let folder_words = vf_extract::matching::meaningful_words(&base);
    let shared = |c: &Client| {
        vf_extract::matching::meaningful_words(&c.name)
            .iter()
            .filter(|w| folder_words.contains(w))
            .count()
    };
    let mut full: Vec<&Client> = list
        .clients
        .iter()
        .filter(|c| vf_extract::matching::name_score(&base, &c.name) >= 0.99)
        .collect();
    if let Some(best) = full.iter().map(|c| shared(c)).max() {
        full.retain(|c| shared(c) == best);
    }
    if full.len() == 1 {
        let c = full[0];
        // Two or more meaningful words on each side (no `LLC`, `Inc`, `The`), about as many on
        // both: `Smith LLC` is not `John Smith`, `Smith Family Trust` is not `Smith`.
        let (fw, cw) = (
            vf_extract::matching::meaningful_words(&base).len(),
            vf_extract::matching::meaningful_words(&c.name).len(),
        );
        let conf = if fw >= 2 && cw >= 2 && fw.abs_diff(cw) <= 1 {
            0.9
        } else {
            0.6
        };
        return (Some(pick(c, false)), "same name".into(), conf);
    }
    if full.len() > 1 {
        let c = full[0];
        return (
            Some(pick(c, false)),
            format!("{} clients have this name", full.len()),
            0.5,
        );
    }
    if let Some(id) = id.filter(|id| looks_like_client_id(id)) {
        // An ID the list does not have: a client to add.
        let c = Client {
            id: id.clone(),
            uuid: list.uuids.get(&id).copied().unwrap_or_else(Uuid::new_v4),
            name: base.clone(),
            extra: Default::default(),
        };
        return (
            Some(pick(&c, true)),
            "new client (ID in the name)".into(),
            0.9,
        );
    }
    if b.status == Status::UnboundFuzzy
        && let Some(c) = b.client_id.as_deref().and_then(|id| list.match_by_id(id))
    {
        return (
            Some(pick(c, false)),
            format!("similar name ({:.0}%)", b.score.unwrap_or_default() * 100.0),
            fuzzy_match(b.score.unwrap_or_default() as f32),
        );
    }
    (None, "no client matches".into(), 0.0)
}

/// A client ID made from a name (owner 2026-11-05, Q-63): its first four letters or digits,
/// upper case (padded with X), then the first number from 1001 not in `taken` (upper case):
/// `1 Farm Poultry LLC` → `1FAR1001`.
pub fn id_from_name(name: &str, taken: &HashSet<String>) -> String {
    let mut base: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(4)
        .collect::<String>()
        .to_ascii_uppercase();
    while base.len() < 4 {
        base.push('X');
    }
    (1001..)
        .map(|n| format!("{base}{n}"))
        .find(|id| !taken.contains(id))
        .expect("a free number")
}

/// The firm template's folder name for `c` (`{Client} ({ID})`), unless it is `rel` already.
pub fn template_name_for(root: &Path, c: &ClientPick, rel: &str) -> Option<String> {
    let (tree, _, err) = template::load_or_seed(&master_dir(root));
    if err.is_some() {
        return None;
    }
    let cl = Client {
        id: c.ext_id.clone(),
        uuid: c.uuid,
        name: c.name.clone(),
        extra: Default::default(),
    };
    template::client_folder_name(&tree, &cl)
        .ok()
        .filter(|n| n != rel)
}

/// Gives every folder that matches no client a new client of its own (Q-63): the folder name
/// as the client's name, an ID made from it ([`id_from_name`]), ticked to bind. Returns how
/// many. The IDs can be changed per row ([`set_new_id`]) before the import.
pub fn new_clients_from_folders(root: &Path, plan: &mut Plan) -> Result<usize, String> {
    let list = client_list(root)?;
    let mut taken: HashSet<String> = list.clients.iter().map(|c| c.id.to_uppercase()).collect();
    taken.extend(
        plan.folders
            .iter()
            .filter_map(|r| r.client.as_ref().map(|c| c.ext_id.to_uppercase())),
    );
    let mut n = 0;
    for r in plan
        .folders
        .iter_mut()
        .filter(|r| !r.bound && r.client.is_none())
    {
        let id = id_from_name(&r.rel, &taken);
        taken.insert(id.clone());
        let c = ClientPick {
            ext_id: id,
            uuid: Uuid::new_v4(),
            name: r.rel.clone(),
            new: true,
        };
        r.template_name = template_name_for(root, &c, &r.rel);
        r.client = Some(c);
        r.how = "new client from the folder name".into();
        r.confidence = 1.0;
        r.include = true;
        n += 1;
    }
    Ok(n)
}

/// Changes the ID of folder row `i`'s new client (typed on the row). Refused when it is empty or
/// another client (in the list or this plan) has it.
pub fn set_new_id(root: &Path, plan: &mut Plan, i: usize, id: &str) -> Result<(), String> {
    let id = id.trim().to_uppercase();
    if id.is_empty() {
        return Err("Type a client ID.".into());
    }
    let list = client_list(root)?;
    if let Some(c) = list.match_by_id(&id) {
        return Err(format!("{id} is {}'s ID already.", c.name));
    }
    if plan.folders.iter().enumerate().any(|(j, r)| {
        j != i
            && r.client
                .as_ref()
                .is_some_and(|c| c.ext_id.eq_ignore_ascii_case(&id))
    }) {
        return Err(format!("{id} is chosen for another folder."));
    }
    let r = plan.folders.get_mut(i).ok_or("no such folder")?;
    let Some(c) = r.client.as_mut().filter(|c| c.new) else {
        return Err("Only a new client's ID can be changed here.".into());
    };
    c.ext_id = id;
    let c = c.clone();
    r.template_name = template_name_for(root, &c, &r.rel);
    Ok(())
}

/// The cabinet's client list (an empty one when no CSV has been imported).
fn client_list(root: &Path) -> Result<ClientList, String> {
    Ok(clients::load(root)?
        .list
        .unwrap_or_else(crate::newclient::empty_list))
}

/// Scans the cabinet's top-level folders and their files, and proposes what each is. Writes
/// nothing.
pub fn plan(conn: &Connection, root: &Path) -> Result<Plan, String> {
    use filer_intake::sentinel::{Progress, ScanInput, Status, scan_root};
    let list = client_list(root)?;
    let (settings, _) = filer_core::config::Settings::load_or_default(&master_dir(root));
    let input = ScanInput {
        root: root.to_owned(),
        clients: std::sync::Arc::new(list.clone()),
        id_pattern: settings.id_pattern.clone(),
        threshold: settings.fuzzy_suggest,
        ignored: vec![],
    };
    let bindings = scan_root(&input, &Progress::default())?;
    let (tree, _, err) = template::load_or_seed(&master_dir(root));
    if let Some(e) = err {
        return Err(e);
    }
    // Clients that already have a folder in the catalog are not proposed again.
    let bound_ids: HashSet<String> = conn
        .prepare("SELECT ext_id FROM client WHERE folder_rel IS NOT NULL AND deleted_at IS NULL")
        .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    let matcher = Matcher::load(conn)?;

    let mut p = Plan {
        clients: list.clients.iter().map(|c| pick(c, false)).collect(),
        ..Plan::default()
    };
    p.clients.sort_by_key(|c| c.name.to_lowercase());
    for b in &bindings {
        let folder_rel = rel(root, &b.path);
        // A readable sentinel binds the folder whether or not its client is in the CSV list
        // (Filer's "orphan": a Filer Desktop tree before the list was imported); the rebuild
        // treats it the same way.
        let bound = matches!(b.status, Status::Bound | Status::Orphan);
        let (mut client, mut how, mut conf) = if bound {
            let c = b
                .client_id
                .as_deref()
                .and_then(|id| list.match_by_id(id))
                .map(|c| pick(c, false))
                .or_else(|| {
                    filer_intake::sentinel::read_sentinel(&b.path)
                        .ok()
                        .flatten()
                        .map(|s| ClientPick {
                            ext_id: s.tax_software_id,
                            uuid: s.client_id,
                            name: s.display_name_at_creation,
                            new: false,
                        })
                });
            (c, "already bound".into(), 1.0)
        } else {
            folder_client(b, &list, &settings.id_pattern)
        };
        if !bound
            && let Some(c) = &client
            && bound_ids.contains(&c.ext_id)
        {
            how = format!("{} already has a folder", c.name);
            conf = conf.min(0.4);
        }
        if conf <= 0.0 {
            client = None;
        }
        let template_name = client.as_ref().and_then(|c| {
            let cl = Client {
                id: c.ext_id.clone(),
                uuid: c.uuid,
                name: c.name.clone(),
                extra: list
                    .match_by_id(&c.ext_id)
                    .map(|l| l.extra.clone())
                    .unwrap_or_default(),
            };
            template::client_folder_name(&tree, &cl)
                .ok()
                .filter(|n| *n != folder_rel)
        });
        p.folders.push(FolderRow {
            rel: folder_rel,
            files: 0,
            bound,
            include: !bound && conf >= AUTO,
            client,
            how,
            confidence: conf,
            template_name,
        });
    }
    // Two folders confidently proposed for the same client (or one and a folder already
    // bound to it): neither goes in on its own. A weak look-alike does not hold back a sure one.
    let mut by_client: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in p.folders.iter().enumerate() {
        if let Some(c) = &r.client
            && (r.bound || r.confidence >= AUTO)
        {
            by_client.entry(c.ext_id.clone()).or_default().push(i);
        }
    }
    for idx in by_client.values().filter(|v| v.len() > 1) {
        for &i in idx {
            let r = &mut p.folders[i];
            if !r.bound {
                r.include = false;
                r.confidence = r.confidence.min(0.5);
                r.how = format!("{} folders point at this client", idx.len());
            }
        }
    }
    let folder_index: HashMap<String, usize> = p
        .folders
        .iter()
        .enumerate()
        .map(|(i, r)| (r.rel.clone(), i))
        .collect();

    let mut paths: Vec<PathBuf> = vec![];
    collect_files(root, &mut paths)?;
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if e.file_type().is_ok_and(|t| t.is_dir())
            && crate::cabinet::is_skipped_name(&n)
            && n != crate::cabinet::VIBE_DIR
        {
            p.skipped.push(n);
        }
    }
    p.skipped.sort();
    for path in paths {
        let r = rel(root, &path);
        let segs: Vec<&str> = r.split('/').collect();
        let (dirs, file) = segs.split_at(segs.len() - 1);
        let folder = dirs.first().and_then(|d| folder_index.get(*d).copied());
        if let Some(i) = folder {
            p.folders[i].files += 1;
        }
        let stem = file[0].rsplit_once('.').map_or(file[0], |(s, _)| s);
        // The client folder's own name says nothing about the doc type or year.
        let inner: &[&str] = if dirs.is_empty() { &[] } else { &dirs[1..] };
        let (t, type_conf, type_how) = matcher.decide(stem, inner);
        let permanent = t.is_some_and(|i| matcher.types[i].2) && type_conf >= AUTO;
        let (tax_year, year_from) = if permanent {
            (None, "permanent")
        } else if let Some(y) = inner.iter().rev().find_map(|d| year_in(d)) {
            (Some(y), "folder")
        } else if let Some(y) = year_in(stem) {
            (Some(y), "file name")
        } else {
            (None, "")
        };
        p.files.push(FileRow {
            rel: r,
            folder,
            tax_year,
            year_from,
            doc_type: t.map(|i| (matcher.types[i].0.clone(), matcher.types[i].1.clone())),
            type_confidence: type_conf,
            type_how,
        });
    }
    Ok(p)
}

/// Every file of the cabinet outside `_x`/`.x`/`~x` top-level folders (Filer's X28 rule) and
/// `_Vibe`.
fn collect_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(root).map_err(|e| format!("cannot read the cabinet: {e}"))?;
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if crate::cabinet::is_skipped_name(&name) || ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            files_under(&e.path(), out).map_err(|e| e.to_string())?;
        } else {
            out.push(e.path());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Apply and undo

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Rename bound folders to the firm template's name (`{Client} ({ID})`).
    pub rename: bool,
    /// Doc type (id) for files the scan found none for (owner 2026-11-05); `None`: left unset.
    pub default_doc_type: Option<String>,
    /// Source (`client`, `firm`, …) for those files; `None`: left as it is.
    pub default_source: Option<String>,
}

/// What [`apply`] did; keep it to [`undo`] the import.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Applied {
    /// Before PA-34: one of each. Kept for an undo record written by an older build.
    #[serde(default)]
    pub bind_tx: Option<String>,
    #[serde(default)]
    pub meta_tx: Option<String>,
    /// One per batch (PA-34).
    #[serde(default)]
    pub bind_txs: Vec<String>,
    #[serde(default)]
    pub meta_txs: Vec<String>,
    pub bound: usize,
    pub renamed: usize,
    pub new_clients: usize,
    pub documents: usize,
    pub mapped: usize,
    /// PDFs queued for OCR (set by the caller: `vf_ocr::queue::enqueue_missing`).
    pub ocr_queued: usize,
    /// Documents whose doc type or year was guessed (tagged `import:guessed`; QA P12).
    #[serde(default)]
    pub guessed: usize,
}

/// `_Vibe/import_last.json`: the last import on this cabinet, for Undo after a restart.
pub fn last_path(root: &Path) -> PathBuf {
    root.join(crate::cabinet::VIBE_DIR).join("import_last.json")
}

pub fn load_last(root: &Path) -> Option<Applied> {
    serde_json::from_slice(&std::fs::read(last_path(root)).ok()?).ok()
}

/// Folders per batch of an import (PA-34).
pub const BATCH: usize = 500;

/// Applies `plan`: binds the included folders, catalogs, sets doc types and years, queues OCR.
pub fn apply(
    conn: &mut Connection,
    root: &Path,
    plan: &Plan,
    opts: &Options,
    device_id: &str,
) -> Result<Applied, String> {
    let (done, total) = (
        std::sync::atomic::AtomicUsize::new(0),
        std::sync::atomic::AtomicUsize::new(0),
    );
    apply_with(conn, root, plan, opts, device_id, &done, &total)
}

/// [`apply`], counting the folders done into `progress` (of `progress_total`).
pub fn apply_with(
    conn: &mut Connection,
    root: &Path,
    plan: &Plan,
    opts: &Options,
    device_id: &str,
    progress: &std::sync::atomic::AtomicUsize,
    progress_total: &std::sync::atomic::AtomicUsize,
) -> Result<Applied, String> {
    let cab = crate::cabinet::read(root)?;
    let mut out = Applied::default();
    // Chosen by hand too: two folders for one client, or a client that has a folder already,
    // would leave duplicate sentinels.
    {
        let mut seen: HashSet<&str> = HashSet::new();
        for r in plan.folders.iter().filter(|r| r.include && !r.bound) {
            let Some(c) = &r.client else { continue };
            if !seen.insert(c.ext_id.as_str()) {
                return Err(format!(
                    "Two folders are chosen for {} ({}): keep one.",
                    c.name, c.ext_id
                ));
            }
            let has: Option<String> = conn
                .query_row(
                    "SELECT folder_rel FROM client WHERE ext_id = ?1 AND deleted_at IS NULL
                       AND folder_rel IS NOT NULL",
                    [&c.ext_id],
                    |r| r.get(0),
                )
                .ok();
            if let Some(f) = has.filter(|f| *f != r.rel) {
                return Err(format!(
                    "{} ({}) has a folder already ({f}): choose another client for {}.",
                    c.name, c.ext_id, r.rel
                ));
            }
        }
    }
    let chosen: Vec<(usize, &FolderRow, &ClientPick)> = plan
        .folders
        .iter()
        .enumerate()
        .filter(|(_, r)| r.include && !r.bound)
        .filter_map(|(i, r)| r.client.as_ref().map(|c| (i, r, c)))
        .collect();

    // New clients go into the master list first (as `newclient::create` does): a crash before
    // the Transaction leaves list entries the next rebuild shows, never a folder half-bound.
    let new: Vec<&ClientPick> = chosen.iter().filter(|c| c.2.new).map(|c| c.2).collect();
    if !new.is_empty() {
        let mut list = client_list(root)?;
        if let Some((c, l)) = new.iter().find_map(|c| {
            list.match_by_id(&c.ext_id)
                .filter(|l| l.uuid != c.uuid)
                .map(|l| (c, l))
        }) {
            return Err(format!(
                "{} is {}'s ID already: give the new client {} another ID.",
                c.ext_id, l.name, c.name
            ));
        }
        for c in &new {
            if list.match_by_id(&c.ext_id).is_none() {
                list.clients.push(Client {
                    id: c.ext_id.clone(),
                    uuid: c.uuid,
                    name: c.name.clone(),
                    extra: Default::default(),
                });
                list.uuids.insert(c.ext_id.clone(), c.uuid);
                out.new_clients += 1;
            }
        }
        clients::save_list(root, &list)?;
    }

    // In batches of folders (PA-34: ~7,600 client folders, ~900K files): each batch binds its
    // folders, catalogs only their files and sets their doc types and years, each step its own
    // Transaction, and the record is saved after it. A stop leaves whole batches done: scanned
    // again, those folders show bound and Import goes on with the rest.
    let chosen_at: HashMap<usize, (&FolderRow, &ClientPick)> =
        chosen.iter().map(|(i, r, c)| (*i, (*r, *c))).collect();
    let mut files_of: HashMap<usize, Vec<&FileRow>> = HashMap::new();
    for f in &plan.files {
        if let Some(i) = f.folder {
            files_of.entry(i).or_default().push(f);
        }
    }
    let work: Vec<usize> = (0..plan.folders.len())
        .filter(|i| chosen_at.contains_key(i) || plan.folders[*i].bound)
        .collect();
    progress_total.store(work.len(), std::sync::atomic::Ordering::Relaxed);
    let mut taken: HashSet<String> = plan.folders.iter().map(|r| r.rel.to_lowercase()).collect();
    for batch in work.chunks(BATCH) {
        // Bind the batch's chosen folders (and rename them, when asked).
        let mut new_rel: HashMap<usize, String> = HashMap::new();
        let mut tx = Transaction::new(LABEL);
        let (mut bound, mut renamed) = (0, 0);
        for &i in batch {
            let Some((r, c)) = chosen_at.get(&i) else {
                continue;
            };
            let mut dest = r.rel.clone();
            if opts.rename
                && let Some(t) = &r.template_name
                && !taken.contains(&t.to_lowercase())
                && !root.join(t).exists()
            {
                tx = tx.file(FileOp::Move {
                    src: root.join(&r.rel),
                    dst: root.join(t),
                });
                taken.insert(t.to_lowercase());
                dest = t.clone();
                renamed += 1;
            }
            tx = tx.file(FileOp::WriteSentinel {
                root: root.to_owned(),
                folder: root.join(&dest),
                client_id: c.uuid.to_string(),
                ext_id: c.ext_id.clone(),
                name: c.name.clone(),
                firm_id: cab.firm_id.clone(),
            });
            new_rel.insert(i, dest);
            bound += 1;
        }
        if bound > 0 {
            tx = tx.audit(AuditEntry::new(
                LABEL,
                None,
                json!({ "folders": bound, "renamed": renamed, "new_clients": out.new_clients }),
            ));
            out.bind_txs
                .push(crate::fileops::commit(conn, root, tx, device_id)?.tx_id);
            out.bound += bound;
            out.renamed += renamed;
        }

        // Catalog rows for the batch's files, attached to their new clients (old names too: a
        // renamed folder's rows are found again as moved).
        let mut scope: Vec<String> = Vec::new();
        for &i in batch {
            scope.push(plan.folders[i].rel.clone());
            if let Some(d) = new_rel.get(&i) {
                scope.push(d.clone());
            }
        }
        rebuild::rebuild_folders(conn, root, device_id, &scope).map_err(|e| e.to_string())?;

        // Each file's doc type and year.
        let mut items: Vec<(String, MetaEdit)> = vec![];
        for &i in batch {
            let r = &plan.folders[i];
            let dest = new_rel.get(&i).unwrap_or(&r.rel);
            for f in files_of.get(&i).map(Vec::as_slice).unwrap_or_default() {
                let rel_now = match f.rel.split_once('/') {
                    Some((_, rest)) => format!("{dest}/{rest}"),
                    None => continue,
                };
                let id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM document WHERE rel_path = ?1 AND deleted_at IS NULL",
                        [&rel_now],
                        |r| r.get(0),
                    )
                    .ok();
                let Some(id) = id else { continue };
                out.documents += 1;
                let mut e = MetaEdit::default();
                if let Some((t, _)) = &f.doc_type
                    && f.type_confidence >= AUTO
                {
                    e.doc_type = Some(Some(t.clone()));
                }
                // No doc type found: the defaults chosen for the scan, if any (a blanket
                // choice, so flagged to check like a guess).
                let mut defaulted = false;
                if e.doc_type.is_none() {
                    if let Some(t) = &opts.default_doc_type {
                        e.doc_type = Some(Some(t.clone()));
                        defaulted = true;
                    }
                    if let Some(s) = &opts.default_source {
                        e.source = Some(Some(s.clone()));
                    }
                }
                match f.year_from {
                    "permanent" => e.tax_year = Some(None),
                    "" => {}
                    _ => e.tax_year = Some(f.tax_year),
                }
                // A year from the file name ("W2 1999 Oak St") or a doc type from a folder
                // name (a "Bank" folder) is a guess: kept out of disposition until someone
                // sets it (QA P12).
                let guessed = (e.tax_year.is_some() && f.year_from == "file name")
                    || (e.doc_type.is_some() && f.type_how.starts_with("folder"))
                    || defaulted;
                if guessed {
                    let mut tags = crate::docmeta::load(conn, &id)?.tags;
                    if !tags.iter().any(|t| t == crate::docmeta::GUESSED_TAG) {
                        tags.push(crate::docmeta::GUESSED_TAG.into());
                    }
                    e.tags = Some(tags);
                    out.guessed += 1;
                }
                if plan.gaps(f).is_empty() {
                    out.mapped += 1;
                }
                if !e.is_empty() {
                    items.push((id, e));
                }
            }
        }
        if !items.is_empty() {
            match crate::docmeta::edit_each(conn, root, &items, META_LABEL, device_id) {
                Ok((tx, _)) => out.meta_txs.push(tx),
                // Every file already had these values (an import run twice).
                Err(e) if e.contains("already have") => {}
                Err(e) => return Err(e),
            }
        }
        progress.fetch_add(batch.len(), std::sync::atomic::Ordering::Relaxed);
        let _ = crate::cabinet::write_replace(
            &last_path(root),
            &serde_json::to_vec_pretty(&out).expect("serializes"),
        );
    }
    let _ = crate::cabinet::write_replace(
        &last_path(root),
        &serde_json::to_vec_pretty(&out).expect("serializes"),
    );
    Ok(out)
}

/// Undoes an import: the doc types and years, then the sentinels and renames; the rebuild then
/// detaches the documents. Clients added to the list stay there (as for a new client's undo).
pub fn undo(
    conn: &mut Connection,
    root: &Path,
    applied: &Applied,
    device_id: &str,
) -> Result<(), String> {
    // Each step once: a retry after one step failed (a folder open in Explorer) skips what was
    // undone already, and the progress is saved.
    let undone = |conn: &Connection, t: &str| -> bool {
        conn.query_row(
            "SELECT undone FROM undo_record WHERE tx_id = ?1",
            [t],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(true)
    };
    let mut left = applied.clone();
    let metas: Vec<String> = applied
        .meta_tx
        .iter()
        .chain(&applied.meta_txs)
        .cloned()
        .collect();
    for t in metas.iter().rev() {
        if !undone(conn, t) {
            crate::fileops::undo(conn, root, t, device_id)?;
        }
        left.meta_tx = None;
        left.meta_txs.retain(|x| x != t);
        let _ = crate::cabinet::write_replace(
            &last_path(root),
            &serde_json::to_vec_pretty(&left).expect("serializes"),
        );
    }
    let binds: Vec<String> = applied
        .bind_tx
        .iter()
        .chain(&applied.bind_txs)
        .cloned()
        .collect();
    for t in binds.iter().rev() {
        if !undone(conn, t) {
            crate::fileops::undo(conn, root, t, device_id)?;
        }
    }
    rebuild::rebuild(conn, root, device_id).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(last_path(root));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_are_found_inside_words() {
        assert_eq!(year_in("2023 Tax Return"), Some(2023));
        assert_eq!(year_in("TY2022"), Some(2022));
        assert_eq!(year_in("Tax Year 2021"), Some(2021));
        assert_eq!(year_in("Stmt 202312"), None);
        assert_eq!(year_in("Box 1234"), None);
        assert_eq!(year_in("2023-12-31 Bank"), Some(2023));
    }

    #[test]
    fn ids_in_folder_names_read_like_filer() {
        let pat = "[A-Z0-9]{4,}";
        assert_eq!(
            id_in_name(pat, "Anderson Farms LLC (ANDE1042)"),
            Some(("ANDE1042".into(), "Anderson Farms LLC".into()))
        );
        assert_eq!(
            id_in_name(pat, "Reyes, Dana_1001"),
            Some(("1001".into(), "Reyes, Dana".into()))
        );
        assert_eq!(id_in_name(pat, "Reyes, Dana"), None);
    }

    #[test]
    fn ids_are_made_from_names() {
        let mut taken = HashSet::new();
        assert_eq!(id_from_name("1 Farm Poultry LLC", &taken), "1FAR1001");
        taken.insert("1FAR1001".to_string());
        assert_eq!(id_from_name("1 Farm Poultry LLC", &taken), "1FAR1002");
        assert_eq!(id_from_name("O'Neil", &taken), "ONEI1001");
        assert_eq!(id_from_name("Al", &taken), "ALXX1001");
    }

    #[test]
    fn match_percentages_follow_how_close_the_names_are() {
        // ID and name: 90-100 %, ticked; closer names higher.
        let (how, full) = id_match(1.0);
        assert_eq!(how.as_deref(), Some("client ID and name"));
        assert!((full - 1.0).abs() < 1e-6);
        let (_, part) = id_match(0.7);
        assert!(part >= 0.9 && part < full && part >= AUTO);
        // The ID is someone else's name: 10-40 %, never ticked, never dropped.
        let (how, off) = id_match(0.3);
        assert!(how.is_none() && off > 0.0 && off <= 0.4);
        assert!(id_match(0.59).1 <= 0.4 && id_match(0.0).1 > 0.0);
        // Only a similar name: below the tick line, closer higher.
        let (lo, hi) = (fuzzy_match(0.85), fuzzy_match(0.97));
        assert!(lo >= 0.6 && lo < hi && hi < AUTO && fuzzy_match(1.0) < AUTO);
    }

    #[test]
    fn words_split_on_punctuation() {
        assert_eq!(words("W-2 Acme, Inc."), ["W", "2", "ACME", "INC"]);
        assert_eq!(words("P&L 2023"), ["P", "&", "L", "2023"]);
    }
}
