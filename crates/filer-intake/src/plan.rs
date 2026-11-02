//! Builds the batch plan: one proposal per queued file (client, rule, year, destination,
//! collision handling, status). Pure apart from read-only disk checks, so it runs on a worker
//! thread and is easy to test.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::clients::{Client, ClientList};
use crate::config;
use crate::intake::QueuedFile;
use crate::parse::{self, Parsed};
use crate::pdf::PageText;
use crate::relationships::Relationships;
use crate::rules::{self, CompiledRules, CopyDest, HitProblem, Resolver, Rule};
use crate::sentinel::{FoldersFile, Status as FolderStatus};
use crate::template::{self, TemplateNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CollisionChoice {
    Overwrite,
    #[default]
    KeepBoth,
    Skip,
}

impl CollisionChoice {
    pub const ALL: [CollisionChoice; 3] = [
        CollisionChoice::KeepBoth,
        CollisionChoice::Overwrite,
        CollisionChoice::Skip,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CollisionChoice::Overwrite => "Overwrite",
            CollisionChoice::KeepBoth => "Keep both",
            CollisionChoice::Skip => "Skip",
        }
    }
}

/// What the user changed on one row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RowEdits {
    pub client_id: Option<String>,
    pub rule_id: Option<String>,
    pub year: Option<u16>,
    pub keep_original_name: bool,
    /// File name typed by the user (Rename, the Combine dialog), instead of the rule's template.
    /// The file keeps its own extension (V5).
    pub filename: Option<String>,
    /// Folder below the client folder picked with "Change folder…", instead of the rule's (V6).
    pub dest_rel: Option<PathBuf>,
    /// Only a client chosen by the user counts (pieces of a split whose client wasn't found:
    /// the ID in the original's file name would be wrong).
    pub manual_client_only: bool,
    pub collision: Option<CollisionChoice>,
    pub skip: bool,
    /// Copy client chosen per copy key (P07a).
    pub copy_clients: BTreeMap<String, String>,
    /// Copies left out of this batch (× on a copy row).
    pub removed_copies: BTreeSet<String>,
    /// Copies added with "+ Copy to…" (client IDs), keyed `added:<n>`.
    pub added_copies: Vec<String>,
    /// Collision choice per copy key (default: the batch choice, X133).
    pub copy_collisions: BTreeMap<String, CollisionChoice>,
    /// For a piece of a split PDF: the original's path. Copies whose rule says `on_split: Whole`
    /// copy the original instead of the piece, once per destination (P09).
    pub split_of: Option<PathBuf>,
}

/// Choices that apply to the whole batch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct BatchSettings {
    /// Used for rows that need a year and have none of their own (sticky, X-spec).
    pub year: Option<u16>,
    pub collision: CollisionChoice,
    /// Rows ticked for a commit of only those (V3). They claim file names first, so they are
    /// numbered " (2)" only against each other; the other rows are numbered after them.
    pub ticked: Vec<PathBuf>,
}

/// Whether the one lazy rescan of this batch has happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rescan {
    NotYet,
    Running,
    /// Done, or not possible (no client list / root): missing folders will be created.
    Done,
}

pub struct PlanInput {
    pub files: Vec<QueuedFile>,
    pub edits: HashMap<PathBuf, RowEdits>,
    pub batch: BatchSettings,
    pub clients: Option<Arc<ClientList>>,
    pub rules: Vec<Rule>,
    pub template: TemplateNode,
    /// Last scan of the saved root, if any.
    pub folders: Option<FoldersFile>,
    pub root: Option<PathBuf>,
    pub id_pattern: String,
    /// Name matches at or above this are offered as a one-click suggestion.
    pub fuzzy_suggest: f32,
    /// Name matches at or above this resolve without a click.
    pub fuzzy_auto: f32,
    pub current_year: i32,
    pub sniff_enabled: bool,
    pub sniff: HashMap<PathBuf, PageText>,
    pub rescan: Rescan,
    /// Split originals some of whose pieces were already filed.
    pub partial: Vec<PathBuf>,
    /// Parent → child relationships, for copies (P07a).
    pub relationships: Arc<Relationships>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMatch {
    Exact,
    /// Name match at or above `fuzzy_auto`, resolved without a click (shown with its score).
    Fuzzy(f64),
    /// Chosen by the user (from suggestions or the picker).
    Manual,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSource {
    Filename,
    /// Matched against the first page's text (only the catch-all matched the filename).
    PageText,
    Override,
    CatchAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YearSource {
    Filename,
    Batch,
    /// Entered on this row.
    Row,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderAction {
    Exists(PathBuf),
    /// Folder with the client's ID in its name, no sentinel yet: bind on commit (X61).
    WillBind(PathBuf),
    WillCreate(PathBuf),
}

impl FolderAction {
    pub fn path(&self) -> &Path {
        match self {
            FolderAction::Exists(p) | FolderAction::WillBind(p) | FolderAction::WillCreate(p) => p,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowStatus {
    Ready,
    Skipped,
    /// Waiting for page text or the folder rescan.
    Pending(String),
    NeedsClient,
    NeedsYear,
    /// The client is bound to two folders (Duplicate in Conflicts): left out of the commit and
    /// kept in the queue until that is resolved.
    Paused,
    Error(String),
}

impl RowStatus {
    /// Rows that keep "Commit" disabled. Paused rows don't: Commit leaves them out (X119).
    pub fn blocks_commit(&self) -> bool {
        !matches!(
            self,
            RowStatus::Ready | RowStatus::Skipped | RowStatus::Paused
        )
    }
}

/// How a copy's client was found.
#[derive(Debug, Clone, PartialEq)]
pub enum CopyMatch {
    /// A child of the primary client (Related resolver).
    Related,
    /// A name from the file name: `related` when it matched one of the primary client's
    /// relationships, otherwise the whole client list (shown as "not related").
    Name {
        score: f64,
        related: bool,
    },
    /// A second client ID in the file name.
    Capture,
    /// Chosen by the user.
    Chosen,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CopyStatus {
    Ready,
    NeedsClient,
    /// Its destination exists and the choice is Skip.
    Skipped,
    Pending(String),
    Error(String),
}

/// One copy of a row's file to another client (P07a).
#[derive(Debug, Clone)]
pub struct CopyProposal {
    /// Stable key for edits: `<copy index>`, `<copy index>:<child id>` or `added:<n>`.
    pub key: String,
    pub template: CopyDest,
    pub role: String,
    pub client: Option<Client>,
    pub client_match: CopyMatch,
    /// Best name match below auto-resolve, offered with one click.
    pub suggestion: Option<(f64, Client)>,
    /// The name or ID read from the file name, when the resolver uses one.
    pub looked_for: Option<String>,
    pub folder: Option<FolderAction>,
    pub dest: Option<PathBuf>,
    pub collision: bool,
    pub collision_with: Option<CollisionWith>,
    pub collision_choice: CollisionChoice,
    pub renamed: bool,
    /// Added with "+ Copy to…".
    pub added: bool,
    /// `on_split: Whole` on a split piece: the unsplit original is copied, not the piece.
    pub whole_of: Option<PathBuf>,
    pub status: CopyStatus,
}

#[derive(Debug, Clone)]
pub struct Proposal {
    pub file: QueuedFile,
    pub parsed: Parsed,
    pub client: Option<Client>,
    pub client_match: ClientMatch,
    /// Best name match between `fuzzy_suggest` and `fuzzy_auto`, offered as a one-click choice
    /// (never applied without that click).
    pub suggestion: Option<(f64, Client)>,
    pub rule_id: Option<String>,
    pub rule_name: String,
    pub rule_source: RuleSource,
    pub year: Option<u16>,
    pub year_source: YearSource,
    pub folder: Option<FolderAction>,
    /// Final full path of the file after filing.
    pub dest: Option<PathBuf>,
    pub dest_dir_exists: bool,
    /// A file is already at the destination (on disk or earlier in this batch).
    pub collision: bool,
    /// What is already there (the hover on the Collision pill, V4).
    pub collision_with: Option<CollisionWith>,
    pub collision_choice: CollisionChoice,
    /// Keep both gave the file a " (n)" name.
    pub renamed: bool,
    pub cross_drive: bool,
    pub status: RowStatus,
    /// Copies to related clients (P07a).
    pub copies: Vec<CopyProposal>,
    /// Next year's folder, created empty with this file (`next_year_folder`, V11).
    pub next_year_dir: Option<PathBuf>,
    /// Notes about copies that were not made (e.g. "no related clients").
    pub copy_notes: Vec<String>,
}

/// What a destination collides with (V4).
#[derive(Debug, Clone, PartialEq)]
pub enum CollisionWith {
    /// A file on disk: its full path, size and last change.
    Disk {
        path: PathBuf,
        bytes: Option<u64>,
        modified: Option<chrono::DateTime<chrono::Local>>,
    },
    /// Another row of this batch (its queued file) goes to the same name first.
    Batch { path: PathBuf, row: PathBuf },
}

fn collision_with(dest: &Path, row: Option<PathBuf>) -> CollisionWith {
    match row {
        Some(row) => CollisionWith::Batch {
            path: dest.to_path_buf(),
            row,
        },
        None => {
            let meta = std::fs::metadata(dest).ok();
            CollisionWith::Disk {
                path: dest.to_path_buf(),
                bytes: meta.as_ref().map(|m| m.len()),
                modified: meta
                    .and_then(|m| m.modified().ok())
                    .map(chrono::DateTime::<chrono::Local>::from),
            }
        }
    }
}

pub struct PlanOutput {
    pub proposals: Vec<Proposal>,
    /// Files whose page text should be read (only the catch-all matched their name).
    pub needs_sniff: Vec<PathBuf>,
    /// Some client has no known folder and the batch hasn't rescanned yet.
    pub needs_rescan: bool,
}

pub fn build_plan(input: &PlanInput) -> PlanOutput {
    let mut out = PlanOutput {
        proposals: Vec::with_capacity(input.files.len()),
        needs_sniff: Vec::new(),
        needs_rescan: false,
    };
    let re = parse::filename_regex(&input.id_pattern);
    let compiled = CompiledRules::new(&input.rules);
    let root_canon = input
        .root
        .as_ref()
        .and_then(|r| std::fs::canonicalize(r).ok());
    // Final paths already taken by earlier rows, lowercased (Windows names are case-insensitive).
    let mut claimed: HashMap<String, PathBuf> = HashMap::new();
    // Ticked rows first (a stable sort keeps the queue order within each group); the
    // proposals still come out in queue order.
    let mut order: Vec<usize> = (0..input.files.len()).collect();
    order.sort_by_key(|&i| !input.batch.ticked.contains(&input.files[i].path));
    let mut planned: Vec<Option<Proposal>> = (0..input.files.len()).map(|_| None).collect();

    for i in order {
        let file = &input.files[i];
        let edits = input.edits.get(&file.path).cloned().unwrap_or_default();
        let name = file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parsed = match &re {
            Ok(re) => parse::parse_filename(&name, re, input.current_year),
            Err(_) => Parsed {
                name: name.clone(),
                id: None,
                rest: name.clone(),
                ext: String::new(),
                year: None,
                stem: name.clone(),
            },
        };
        let mut p = Proposal {
            file: file.clone(),
            parsed,
            client: None,
            client_match: ClientMatch::None,
            suggestion: None,
            rule_id: None,
            rule_name: String::new(),
            rule_source: RuleSource::CatchAll,
            year: None,
            year_source: YearSource::None,
            folder: None,
            dest: None,
            dest_dir_exists: false,
            collision: false,
            collision_with: None,
            collision_choice: edits.collision.unwrap_or(input.batch.collision),
            renamed: false,
            cross_drive: false,
            status: RowStatus::Ready,
            copies: Vec::new(),
            copy_notes: Vec::new(),
            next_year_dir: None,
        };
        // A skipped row is still planned (to show where it would go) but claims no names, so
        // the rows after it are not numbered because of it.
        let mut scratch;
        let claims = if edits.skip {
            scratch = claimed.clone();
            &mut scratch
        } else {
            &mut claimed
        };
        let status = plan_row(
            input,
            &compiled,
            root_canon.as_deref(),
            &edits,
            &mut p,
            claims,
            &mut out,
        );
        p.status = if edits.skip {
            RowStatus::Skipped
        } else {
            status
        };
        planned[i] = Some(p);
    }
    out.proposals = planned.into_iter().flatten().collect();
    out
}

/// `b` exists with the same bytes as `a` (size first, then SHA-256).
fn same_bytes(a: &Path, b: &Path) -> bool {
    let size = |p: &Path| std::fs::metadata(p).ok().map(|m| m.len());
    match (size(a), size(b)) {
        (Some(x), Some(y)) if x == y => {
            matches!((crate::commit::sha256_file(a), crate::commit::sha256_file(b)), (Ok(h1), Ok(h2)) if h1 == h2)
        }
        _ => false,
    }
}

/// Fills `p` and returns its status. Stops at the first thing that blocks the row.
fn plan_row(
    input: &PlanInput,
    compiled: &Result<CompiledRules, String>,
    root_canon: Option<&Path>,
    edits: &RowEdits,
    p: &mut Proposal,
    claimed: &mut HashMap<String, PathBuf>,
    out: &mut PlanOutput,
) -> RowStatus {
    if let Err(e) = parse::filename_regex(&input.id_pattern) {
        return RowStatus::Error(format!("The client ID pattern is not valid: {e}"));
    }
    let compiled = match compiled {
        Ok(c) => c,
        Err(e) => return RowStatus::Error(format!("Fix the rules first: {e}")),
    };
    if input.partial.contains(&p.file.path) {
        return RowStatus::Error(
            "Some pages of this PDF were already filed as split pieces. Split it again and skip those pieces, or file the rest by hand."
                .into(),
        );
    }

    // Client.
    if let Some(list) = &input.clients {
        let by_id = |id: &str| list.match_by_id(id).cloned();
        if let Some(c) = edits.client_id.as_deref().and_then(by_id) {
            p.client = Some(c);
            p.client_match = ClientMatch::Manual;
        } else if edits.manual_client_only {
            // No automatic match.
        } else if let Some(c) = p.parsed.id.as_deref().and_then(by_id) {
            p.client = Some(c);
            p.client_match = ClientMatch::Exact;
        } else if let Some((score, c)) = list.fuzzy_by_name(&p.parsed.name, 1).first() {
            if *score >= f64::from(input.fuzzy_auto) {
                p.client = Some((*c).clone());
                p.client_match = ClientMatch::Fuzzy(*score);
            } else if *score >= f64::from(input.fuzzy_suggest) {
                p.suggestion = Some((*score, (*c).clone()));
            }
        }
    }

    // Rule.
    let override_rule = edits
        .rule_id
        .as_deref()
        .and_then(|id| input.rules.iter().find(|r| r.id == id));
    let rule: &Rule = if let Some(r) = override_rule {
        p.rule_source = RuleSource::Override;
        r
    } else if let Some(r) = compiled.first_specific_match(&p.parsed.rest) {
        p.rule_source = RuleSource::Filename;
        r
    } else {
        // Page text only for PDFs (zip-routed files may be any type, P07b).
        let is_pdf = p.file.kind == crate::intake::Kind::Pdf;
        let from_text = if input.sniff_enabled && is_pdf {
            match input.sniff.get(&p.file.path) {
                Some(PageText::Text(t)) => compiled.first_specific_match(t),
                Some(PageText::Failed(_)) => None,
                Some(PageText::Pending) | None => {
                    out.needs_sniff.push(p.file.path.clone());
                    p.rule_name = rules::CATCH_ALL_NAME.into();
                    return RowStatus::Pending("Reading page text…".into());
                }
            }
        } else {
            None
        };
        match from_text {
            Some(r) => {
                p.rule_source = RuleSource::PageText;
                r
            }
            None => {
                p.rule_source = RuleSource::CatchAll;
                compiled.first_match(&p.parsed.rest)
            }
        }
    };
    p.rule_id = Some(rule.id.clone());
    p.rule_name = rule.name.clone();

    // Year.
    (p.year, p.year_source) = match (edits.year, p.parsed.year, input.batch.year) {
        (Some(y), _, _) => (Some(y), YearSource::Row),
        (None, Some(y), _) => (Some(y), YearSource::Filename),
        (None, None, Some(y)) => (Some(y), YearSource::Batch),
        _ => (None, YearSource::None),
    };

    let Some(client) = p.client.clone() else {
        return RowStatus::NeedsClient;
    };
    if has_duplicate_folders(input, &client) {
        return RowStatus::Paused;
    }
    // Copies (P07a), resolved before the primary, whose name may use {Copy.*}. Named groups are
    // read from the file name only (X136).
    if let Some(list) = &input.clients {
        let captures = match p.rule_source {
            RuleSource::Filename | RuleSource::Override => {
                compiled.captures(&rule.id, &p.parsed.rest)
            }
            _ => HashMap::new(),
        };
        let (mut copies, notes) = resolve_copies(input, list, rule, &client, edits, &captures);
        if let Some(original) = &edits.split_of {
            for c in copies.iter_mut().filter(|c| !c.added) {
                if c.template.on_split == rules::OnSplit::Whole {
                    c.whole_of = Some(original.clone());
                }
            }
        }
        p.copies = copies;
        p.copy_notes = notes;
        if p.copies
            .iter()
            .filter_map(|c| c.client.as_ref())
            .any(|c| has_duplicate_folders(input, c))
        {
            return RowStatus::Paused;
        }
    }
    let first = (rule.copies.len() == 1)
        .then(|| p.copies.iter().find(|c| !c.added))
        .flatten();
    // An optional copy that found no client: {Copy.Client} is the text that was looked for.
    let looked_for = first
        .filter(|c| c.client.is_none() && c.status == CopyStatus::Skipped)
        .and_then(|c| c.looked_for.as_deref())
        .map(|name| Client {
            id: String::new(),
            uuid: uuid::Uuid::nil(),
            name: clean_captured_name(name),
            extra: Default::default(),
        });
    let first_copy = first
        .and_then(|c| c.client.as_ref().map(|cl| (cl, c.role.as_str())))
        .or(looked_for.as_ref().map(|cl| (cl, "")));
    // A folder picked by hand needs no year when the file name doesn't either (V6): a rule
    // without copies whose name is typed, kept, or has no {Year}.
    let relaxed;
    let eval_rule = if edits.dest_rel.is_some()
        && p.year.is_none()
        && rule.requires_year
        && p.copies.is_empty()
        && (edits.keep_original_name
            || edits.filename.is_some()
            || !template::tokens_in(&rule.filename_template)
                .unwrap_or_default()
                .iter()
                .any(|t| t == "Year"))
    {
        let named = edits.keep_original_name || edits.filename.is_some();
        relaxed = Rule {
            requires_year: false,
            dest_template: "{Client}".into(),
            // Replaced by the typed or original name below.
            filename_template: if named {
                "{Rest}.{Ext}".into()
            } else {
                rule.filename_template.clone()
            },
            ..rule.clone()
        };
        &relaxed
    } else {
        rule
    };
    let rendered =
        match rules::evaluate_rule_with_copy(eval_rule, &p.parsed, &client, p.year, first_copy)
            .outcome
        {
            Ok(r) => r,
            Err(HitProblem::NeedsYear) => return RowStatus::NeedsYear,
            Err(HitProblem::Render(e)) => {
                return RowStatus::Error(format!("Cannot build the name: {e}."));
            }
        };
    let Some(root) = &input.root else {
        return RowStatus::Error("Set the client root in Setup.".into());
    };

    // Folder.
    let folder = match find_folder(input, &client) {
        Some(f) => f,
        None if input.rescan != Rescan::Done => {
            out.needs_rescan = true;
            return RowStatus::Pending("Looking for the client's folder…".into());
        }
        None => {
            let name = match template::client_folder_name(&input.template, &client) {
                Ok(n) => n,
                Err(e) => return RowStatus::Error(format!("Cannot name the client folder: {e}.")),
            };
            let path = root.join(&name);
            if path.exists() {
                return RowStatus::Error(format!(
                    "A folder named \"{name}\" already exists but isn't bound to this client. Bind it in Conflicts."
                ));
            }
            FolderAction::WillCreate(path)
        }
    };
    let dest_dir = match &edits.dest_rel {
        Some(rel) => match checked_rel(rel)
            .and_then(|rel| template::checked_join(folder.path(), &rel).map_err(|e| e.to_string()))
        {
            Ok(d) => d,
            Err(e) => return RowStatus::Error(format!("Folder: {e}.")),
        },
        None => folder.path().join(&rendered.dest_rel),
    };
    p.folder = Some(folder);
    // "Original name" wins over a typed name; a typed name keeps the file's extension.
    let filename = if edits.keep_original_name {
        p.file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(rendered.filename)
    } else if let Some(name) = &edits.filename {
        let mut name = match template::safe_file_name(name) {
            Ok(n) => n,
            Err(e) => return RowStatus::Error(format!("File name: {e}.")),
        };
        let same_ext = Path::new(&name)
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(&p.parsed.ext));
        if !same_ext && !p.parsed.ext.is_empty() {
            name = format!("{name}.{}", p.parsed.ext);
        }
        name
    } else {
        rendered.filename
    };
    let mut dest = match template::checked_join(&dest_dir, Path::new(&filename)) {
        Ok(d) => d,
        Err(e) => return RowStatus::Error(format!("{e}.")),
    };
    if same_file(&dest, &p.file.path) {
        return RowStatus::Error("The file is already at its destination.".into());
    }

    // Collision.
    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    let in_batch = claimed.get(&key(&dest)).cloned();
    if in_batch.is_some() || dest.exists() {
        p.collision = true;
        p.collision_with = Some(collision_with(&dest, in_batch.clone()));
        // Two files of this batch on one name are always numbered, never overwritten.
        let choice = if in_batch.is_some() {
            CollisionChoice::KeepBoth
        } else {
            p.collision_choice
        };
        p.collision_choice = choice;
        match choice {
            CollisionChoice::Skip => {
                p.dest = Some(dest);
                return RowStatus::Skipped;
            }
            CollisionChoice::Overwrite => {}
            CollisionChoice::KeepBoth => {
                dest = match free_name(&dest, |c| claimed.contains_key(&key(c)) || c.exists()) {
                    Ok(d) => d,
                    Err(e) => return RowStatus::Error(format!("{e}.")),
                };
                p.renamed = true;
            }
        }
    }
    claimed.insert(key(&dest), p.file.path.clone());
    // Like the old filer: a year folder also gets next year's, empty (V11).
    p.next_year_dir = match (rule.next_year_folder, p.year, &edits.dest_rel) {
        (true, Some(y), None) => {
            rules::evaluate_rule_with_copy(rule, &p.parsed, &client, Some(y + 1), first_copy)
                .outcome
                .ok()
                .zip(p.folder.as_ref())
                .map(|(r, f)| f.path().join(r.dest_rel))
                .filter(|d| *d != dest_dir && !d.is_dir())
        }
        _ => None,
    };
    p.dest_dir_exists = dest_dir.is_dir();
    p.cross_drive = match (std::fs::canonicalize(&p.file.path).ok(), root_canon) {
        (Some(f), Some(r)) => !config::same_volume(&f, r),
        _ => !config::same_volume(&p.file.path, root),
    };
    p.dest = Some(dest);
    plan_copy_paths(input, root, rule, &client, edits, p, claimed, out)
}

/// The copy's client from the file name (RelatedByName): the primary client's related clients
/// first (≥ 0.85), then the whole list with the usual thresholds.
fn resolve_by_name(
    input: &PlanInput,
    list: &ClientList,
    primary: &Client,
    name: &str,
) -> (Option<Client>, CopyMatch, Option<(f64, Client)>, String) {
    const RELATED_BAR: f64 = 0.85;
    let name = clean_captured_name(name);
    let rel = &input.relationships;
    let related = rel
        .children(&primary.id, &[])
        .into_iter()
        .map(|r| (r.child_id.as_str(), r.role.as_str()))
        .chain(
            rel.parents(&primary.id, &[])
                .into_iter()
                .map(|r| (r.parent_id.as_str(), r.role.as_str())),
        )
        .filter_map(|(id, role)| list.match_by_id(id).map(|c| (c, role)));
    let best = related
        .map(|(c, role)| (crate::clients::name_score(&name, &c.name), c, role))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((score, c, role)) = best
        && score >= RELATED_BAR
    {
        return (
            Some(c.clone()),
            CopyMatch::Name {
                score,
                related: true,
            },
            None,
            role.to_string(),
        );
    }
    match list.fuzzy_by_name(&name, 1).first() {
        Some((score, c)) if *score >= f64::from(input.fuzzy_auto) => (
            Some((*c).clone()),
            CopyMatch::Name {
                score: *score,
                related: false,
            },
            None,
            String::new(),
        ),
        Some((score, c)) if *score >= f64::from(input.fuzzy_suggest) => (
            None,
            CopyMatch::None,
            Some((*score, (*c).clone())),
            String::new(),
        ),
        _ => (None, CopyMatch::None, None, String::new()),
    }
}

/// A captured name without a trailing year or other digits ("Baker, Thomas 2025" → "Baker, Thomas").
fn clean_captured_name(name: &str) -> String {
    name.trim()
        .trim_end_matches(|c: char| {
            c.is_ascii_digit() || c.is_whitespace() || matches!(c, '-' | '_' | '.' | ',')
        })
        .trim()
        .to_string()
}

/// Every copy of the row, with its client resolved (or not) but no paths yet.
fn resolve_copies(
    input: &PlanInput,
    list: &ClientList,
    rule: &Rule,
    client: &Client,
    edits: &RowEdits,
    captures: &HashMap<String, String>,
) -> (Vec<CopyProposal>, Vec<String>) {
    let mut out: Vec<CopyProposal> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let blank = |key: String, template: &CopyDest| CopyProposal {
        key,
        template: template.clone(),
        role: String::new(),
        client: None,
        client_match: CopyMatch::None,
        suggestion: None,
        looked_for: None,
        folder: None,
        dest: None,
        collision: false,
        collision_with: None,
        collision_choice: CollisionChoice::default(),
        renamed: false,
        added: false,
        whole_of: None,
        status: CopyStatus::NeedsClient,
    };
    for (ci, cd) in rule.copies.iter().enumerate() {
        match &cd.resolver {
            Resolver::Related { roles } => {
                let kids = input.relationships.children(&client.id, roles);
                if kids.is_empty() {
                    notes.push("no related clients".into());
                }
                for r in kids {
                    let key = format!("{ci}:{}", r.child_id);
                    if edits.removed_copies.contains(&key) {
                        continue;
                    }
                    match list.match_by_id(&r.child_id) {
                        Some(c) => out.push(CopyProposal {
                            role: r.role.clone(),
                            client: Some(c.clone()),
                            client_match: CopyMatch::Related,
                            status: CopyStatus::Ready,
                            ..blank(key, cd)
                        }),
                        None => notes.push(format!(
                            "related client {} is not in the client list",
                            r.child_id
                        )),
                    }
                }
            }
            Resolver::RelatedByName { group } => {
                let key = ci.to_string();
                if edits.removed_copies.contains(&key) {
                    continue;
                }
                let mut c = blank(key, cd);
                if let Some(name) = captures.get(group).filter(|n| !n.is_empty()) {
                    let (found, how, suggestion, role) = resolve_by_name(input, list, client, name);
                    c.looked_for = Some(name.clone());
                    c.client_match = how;
                    c.suggestion = suggestion;
                    c.role = role;
                    if let Some(f) = found {
                        c.client = Some(f);
                        c.status = CopyStatus::Ready;
                    } else if cd.optional && c.suggestion.is_none() {
                        // Not a client's name (e.g. "K-1 summary"): no copy, no question.
                        c.status = CopyStatus::Skipped;
                        notes.push(format!("\"{}\" is not a client, so no copy", name.trim()));
                    }
                }
                out.push(c);
            }
            Resolver::Capture { group } => {
                let key = ci.to_string();
                if edits.removed_copies.contains(&key) {
                    continue;
                }
                let mut c = blank(key, cd);
                if let Some(id) = captures.get(group).filter(|n| !n.is_empty()) {
                    c.looked_for = Some(id.clone());
                    if let Some(f) = list.match_by_id(id) {
                        c.client = Some(f.clone());
                        c.client_match = CopyMatch::Capture;
                        c.status = CopyStatus::Ready;
                    }
                }
                out.push(c);
            }
            Resolver::Manual => {
                let key = ci.to_string();
                if !edits.removed_copies.contains(&key) {
                    out.push(blank(key, cd));
                }
            }
        }
    }
    // "+ Copy to…" copies use the rule's first copy templates (or the default ones).
    let template = rule
        .copies
        .first()
        .cloned()
        .unwrap_or_else(|| CopyDest::new(Resolver::Manual));
    for (n, id) in edits.added_copies.iter().enumerate() {
        let key = format!("added:{n}");
        if edits.removed_copies.contains(&key) {
            continue;
        }
        if let Some(c) = list.match_by_id(id) {
            out.push(CopyProposal {
                client: Some(c.clone()),
                client_match: CopyMatch::Chosen,
                added: true,
                status: CopyStatus::Ready,
                ..blank(key, &template)
            });
        }
    }
    // A client chosen on the copy row wins.
    for c in &mut out {
        if let Some(id) = edits.copy_clients.get(&c.key)
            && let Some(chosen) = list.match_by_id(id)
        {
            c.client = Some(chosen.clone());
            c.client_match = CopyMatch::Chosen;
            c.suggestion = None;
            c.status = CopyStatus::Ready;
        }
    }
    // A copy to the primary client itself is left out (X136).
    out.retain(|c| {
        let own = c.client.as_ref().is_some_and(|x| x.id == client.id);
        if own {
            notes.push(format!("a copy to {} itself was left out", client.name));
        }
        !own
    });
    (out, notes)
}

/// Folders, paths and collisions for the row's resolved copies; the row's status from them.
#[allow(clippy::too_many_arguments)]
fn plan_copy_paths(
    input: &PlanInput,
    root: &Path,
    rule: &Rule,
    client: &Client,
    edits: &RowEdits,
    p: &mut Proposal,
    claimed: &mut HashMap<String, PathBuf>,
    out: &mut PlanOutput,
) -> RowStatus {
    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    for c in &mut p.copies {
        let Some(cc) = c.client.clone() else { continue };
        let rendered = match rules::evaluate_copy(
            rule,
            &c.template,
            &p.parsed,
            client,
            p.year,
            &cc,
            &c.role,
        ) {
            Ok(r) => r,
            Err(HitProblem::NeedsYear) => return RowStatus::NeedsYear,
            Err(HitProblem::Render(e)) => {
                c.status = CopyStatus::Error(format!("Cannot build the copy's name: {e}."));
                continue;
            }
        };
        let folder = match find_folder(input, &cc) {
            Some(f) => f,
            None if input.rescan != Rescan::Done => {
                out.needs_rescan = true;
                c.status = CopyStatus::Pending("Looking for the client's folder…".into());
                continue;
            }
            None => match template::client_folder_name(&input.template, &cc) {
                Ok(name) if root.join(&name).exists() => {
                    c.status = CopyStatus::Error(format!(
                        "A folder named \"{name}\" already exists but isn't bound to {}. Bind it in Conflicts.",
                        cc.name
                    ));
                    continue;
                }
                Ok(name) => FolderAction::WillCreate(root.join(name)),
                Err(e) => {
                    c.status = CopyStatus::Error(format!("Cannot name the client folder: {e}."));
                    continue;
                }
            },
        };
        let dir = folder.path().join(&rendered.dest_rel);
        c.folder = Some(folder);
        let mut dest = match template::checked_join(&dir, Path::new(&rendered.filename)) {
            Ok(d) => d,
            Err(e) => {
                c.status = CopyStatus::Error(format!("{e}."));
                continue;
            }
        };
        c.collision_choice = edits
            .copy_collisions
            .get(&c.key)
            .copied()
            .unwrap_or(input.batch.collision);
        // The whole original goes to each destination once, with the first piece that has it,
        // and not again in a later batch (a split filed in two goes) when it is already there.
        if let Some(original) = &c.whole_of
            && (claimed
                .insert(
                    format!("whole|{}|{}", key(original), key(&dest)),
                    p.file.path.clone(),
                )
                .is_some()
                || same_bytes(original, &dest))
        {
            c.status = CopyStatus::Skipped;
            c.dest = None;
            continue;
        }
        let in_batch = claimed.get(&key(&dest)).cloned();
        if in_batch.is_some() || dest.exists() {
            c.collision = true;
            c.collision_with = Some(collision_with(&dest, in_batch.clone()));
            if in_batch.is_some() {
                c.collision_choice = CollisionChoice::KeepBoth;
            }
            match c.collision_choice {
                CollisionChoice::Skip => {
                    c.dest = Some(dest);
                    c.status = CopyStatus::Skipped;
                    continue;
                }
                CollisionChoice::Overwrite => {}
                CollisionChoice::KeepBoth => {
                    dest = match free_name(&dest, |x| claimed.contains_key(&key(x)) || x.exists()) {
                        Ok(d) => d,
                        Err(e) => {
                            c.status = CopyStatus::Error(format!("{e}."));
                            continue;
                        }
                    };
                    c.renamed = true;
                }
            }
        }
        claimed.insert(key(&dest), p.file.path.clone());
        c.dest = Some(dest);
        c.status = CopyStatus::Ready;
    }
    let before = p.copies.len();
    p.copies
        .retain(|c| !(c.whole_of.is_some() && c.dest.is_none() && c.status == CopyStatus::Skipped));
    if p.copies.len() < before {
        p.copy_notes
            .push("the whole original already goes there (with an earlier piece or batch)".into());
    }
    // The row is ready only when every copy is.
    for c in &p.copies {
        match &c.status {
            CopyStatus::NeedsClient => return RowStatus::NeedsClient,
            CopyStatus::Pending(why) => return RowStatus::Pending(why.clone()),
            CopyStatus::Error(e) => {
                let who = c.client.as_ref().map_or("a client", |x| x.name.as_str());
                return RowStatus::Error(format!("Copy to {who}: {e}"));
            }
            CopyStatus::Ready | CopyStatus::Skipped => {}
        }
    }
    RowStatus::Ready
}

/// The client's bound folder, or an ID-named folder to bind. `None` if neither is known.
/// A folder typed or picked below the client folder: relative, no `..`, each part a name
/// Windows can hold (V6).
pub fn checked_rel(rel: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(part) => {
                let part = part.to_string_lossy();
                let safe = template::safe_file_name(&part).map_err(|e| e.to_string())?;
                if safe != part {
                    return Err(format!("\"{part}\" can't be a folder name"));
                }
                out.push(safe);
            }
            std::path::Component::CurDir => {}
            _ => return Err("use a folder inside the client folder".into()),
        }
    }
    Ok(out)
}

fn find_folder(input: &PlanInput, client: &Client) -> Option<FolderAction> {
    let scan = input.folders.as_ref()?;
    let mine =
        |b: &&crate::sentinel::FolderBinding| b.client_id.as_deref() == Some(client.id.as_str());
    if let Some(b) = scan
        .folders
        .iter()
        .filter(mine)
        .find(|b| b.status == FolderStatus::Bound)
    {
        // A bound folder that has since vanished counts as unknown (triggers a rescan).
        return b
            .path
            .is_dir()
            .then(|| FolderAction::Exists(b.path.clone()));
    }
    scan.folders
        .iter()
        .filter(mine)
        .find(|b| b.status == FolderStatus::UnboundNamed && b.path.is_dir())
        .map(|b| FolderAction::WillBind(b.path.clone()))
}

/// The client's sentinel is in two or more folders (Duplicate in the last scan).
fn has_duplicate_folders(input: &PlanInput, client: &Client) -> bool {
    input.folders.as_ref().is_some_and(|scan| {
        scan.folders.iter().any(|b| {
            b.status == FolderStatus::Duplicate
                && b.client_id.as_deref() == Some(client.id.as_str())
        })
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// `name (2).ext`, `name (3).ext`, ... — the first one `taken` says is free.
fn free_name(path: &Path, taken: impl Fn(&Path) -> bool) -> Result<PathBuf, template::RenderError> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let dir = path.parent().unwrap_or(Path::new(""));
    for n in 2.. {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = template::checked_join(dir, Path::new(&name))?;
        if !taken(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("unbounded range")
}

/// Totals for the Commit summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub moves: usize,
    pub copies: usize,
    pub new_client_folders: usize,
    pub binds: usize,
    pub new_sub_folders: usize,
    pub overwrites: usize,
    pub renamed: usize,
    pub skipped: usize,
    /// Rows left out because their client is bound to two folders.
    pub paused: usize,
    /// Copies to related clients (P07a) in ready rows.
    pub related_copies: usize,
}

pub fn summarize<'a>(proposals: impl IntoIterator<Item = &'a Proposal>) -> Summary {
    let mut s = Summary::default();
    let mut created: HashSet<&Path> = HashSet::new();
    let mut bound: HashSet<&Path> = HashSet::new();
    let mut subdirs: HashSet<PathBuf> = HashSet::new();
    for p in proposals {
        match p.status {
            RowStatus::Skipped => {
                s.skipped += 1;
                continue;
            }
            RowStatus::Paused => {
                s.paused += 1;
                continue;
            }
            RowStatus::Ready => {}
            _ => continue,
        }
        s.files += 1;
        if p.cross_drive {
            s.copies += 1;
        } else {
            s.moves += 1;
        }
        match &p.folder {
            Some(FolderAction::WillCreate(f)) => {
                created.insert(f);
            }
            Some(FolderAction::WillBind(f)) => {
                bound.insert(f);
            }
            _ => {}
        }
        if !p.dest_dir_exists
            && let Some(d) = p.dest.as_ref().and_then(|d| d.parent())
        {
            subdirs.insert(d.to_path_buf());
        }
        if p.collision && p.collision_choice == CollisionChoice::Overwrite {
            s.overwrites += 1;
        }
        if p.renamed {
            s.renamed += 1;
        }
        for c in p.copies.iter().filter(|c| c.status == CopyStatus::Ready) {
            s.related_copies += 1;
            match &c.folder {
                Some(FolderAction::WillCreate(f)) => {
                    created.insert(f);
                }
                Some(FolderAction::WillBind(f)) => {
                    bound.insert(f);
                }
                _ => {}
            }
            if c.collision && c.collision_choice == CollisionChoice::Overwrite {
                s.overwrites += 1;
            }
            if c.renamed {
                s.renamed += 1;
            }
        }
    }
    s.new_client_folders = created.len();
    s.binds = bound.len();
    s.new_sub_folders = subdirs.len();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColumnMap;
    use crate::intake::{Kind, Source};
    use crate::sentinel::FolderBinding;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        inbox: PathBuf,
        clients: Arc<ClientList>,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();
        let csv = tmp.path().join("c.csv");
        std::fs::write(
            &csv,
            "id,name\n1001,\"Smith, John\"\n1002,Acme LLC\n1003,\"Jones, Mary\"\n1004,Baker Farms\n",
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
        Fixture {
            _tmp: tmp,
            root,
            inbox,
            clients,
        }
    }

    fn queued(path: &Path) -> QueuedFile {
        QueuedFile {
            path: path.to_path_buf(),
            size: 10,
            added: chrono::Local::now(),
            source: Source::Manual,
            kind: Kind::Pdf,
        }
    }

    fn input(f: &Fixture, names: &[&str]) -> PlanInput {
        let files = names
            .iter()
            .map(|n| {
                let p = f.inbox.join(n);
                std::fs::write(&p, b"%PDF").unwrap();
                queued(&p)
            })
            .collect();
        PlanInput {
            files,
            edits: HashMap::new(),
            batch: BatchSettings::default(),
            clients: Some(f.clients.clone()),
            rules: rules::tax_rules(),
            template: template::default_tree(),
            folders: Some(FoldersFile {
                root: f.root.clone(),
                scanned_at: chrono::Utc::now(),
                folders: vec![],
                ignored: vec![],
            }),
            root: Some(f.root.clone()),
            id_pattern: parse::DEFAULT_ID_PATTERN.into(),
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

    fn bind(
        f: &Fixture,
        inp: &mut PlanInput,
        name: &str,
        id: &str,
        status: FolderStatus,
    ) -> PathBuf {
        let p = f.root.join(name);
        std::fs::create_dir_all(&p).unwrap();
        inp.folders.as_mut().unwrap().folders.push(FolderBinding {
            path: p.clone(),
            client_id: Some(id.into()),
            status,
            score: None,
            note: None,
        });
        p
    }

    fn row<'a>(out: &'a PlanOutput, name: &str) -> &'a Proposal {
        out.proposals
            .iter()
            .find(|p| p.file.path.file_name().unwrap() == name)
            .unwrap()
    }

    #[test]
    fn exact_id_bound_folder_ready() {
        let f = fixture();
        let mut inp = input(&f, &["Smith, John_1001_2025_1040.pdf"]);
        let folder = bind(
            &f,
            &mut inp,
            "Smith, John (1001)",
            "1001",
            FolderStatus::Bound,
        );
        let out = build_plan(&inp);
        let p = &out.proposals[0];
        assert_eq!(p.status, RowStatus::Ready);
        assert_eq!(p.client_match, ClientMatch::Exact);
        assert_eq!(p.rule_name, "Tax Return");
        assert_eq!(p.year_source, YearSource::Filename);
        assert_eq!(p.folder, Some(FolderAction::Exists(folder.clone())));
        assert_eq!(
            p.dest.as_deref(),
            Some(
                folder
                    .join("Tax Returns")
                    .join("2025")
                    .join("2025 Tax Return - Smith, John.pdf")
                    .as_path()
            )
        );
        assert!(!p.dest_dir_exists);
        assert!(!p.cross_drive);
    }

    /// A client list and relationships for the copy (P07a) tests.
    fn copy_input(f: &Fixture, files: &[&str], rels: &[(&str, &str, &str)]) -> PlanInput {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("c.csv");
        std::fs::write(
            &csv,
            "id,name
4472,Riverbend Holdings LP
2093,\"Baker, Thomas\"
1042,\"Hendricks, Mark\"
4471,Whitfield Farms LLC
1234,Entity Co
",
        )
        .unwrap();
        let table = crate::clients::read_csv(&csv).unwrap();
        let map = ColumnMap {
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec![],
        };
        let mut inp = input(f, files);
        inp.clients = Some(Arc::new(
            crate::clients::apply_mapping(&table, &map).unwrap(),
        ));
        let mut r = Relationships::default();
        for (p, c, role) in rels {
            r.add(crate::relationships::Relationship {
                parent_id: p.to_string(),
                child_id: c.to_string(),
                role: role.to_string(),
                note: String::new(),
            })
            .unwrap();
        }
        inp.relationships = Arc::new(r);
        inp.batch.year = Some(2025);
        inp
    }

    fn copy_of<'a>(out: &'a PlanOutput, file: &str) -> &'a [CopyProposal] {
        &row(out, file).copies
    }

    #[test]
    fn related_by_name_resolver_cases() {
        let f = fixture();
        let related = "Riverbend_4472_K-1 Tom Baker.pdf"; // 0.89 to Baker, Thomas
        let far = "Riverbend_4472_K-1 Thomas Bakker.pdf"; // 0.98, not related
        let suggest = "Riverbend_4472_K-1 Hendrix Mark.pdf"; // 0.90, not related
        let none = "Riverbend_4472_K-1 Zzz Qqq.pdf";
        // Baker is related to Riverbend; Hendricks is not.
        let inp = copy_input(
            &f,
            &[related, far, suggest, none],
            &[("4472", "2093", "partner")],
        );
        let out = build_plan(&inp);

        let c = &copy_of(&out, related)[0];
        assert_eq!(c.client.as_ref().unwrap().id, "2093");
        assert!(matches!(
            c.client_match,
            CopyMatch::Name { related: true, .. }
        ));
        assert_eq!(c.role, "partner");
        assert_eq!(row(&out, related).status, RowStatus::Ready);
        // The primary name uses {Copy.Client}; the copy goes to Baker's folder.
        assert!(
            row(&out, related)
                .dest
                .as_ref()
                .unwrap()
                .ends_with("2025 K-1 - Baker, Thomas.pdf")
        );
        assert!(
            c.dest.as_ref().unwrap().ends_with(
                std::path::Path::new("Baker, Thomas (2093)")
                    .join("Source Documents")
                    .join("2025")
                    .join("2025 K-1 from Riverbend Holdings LP.pdf")
            )
        );

        // Bakker resolves to Baker too (≥ 0.97) — the related step catches it first here.
        assert_eq!(copy_of(&out, far)[0].client.as_ref().unwrap().id, "2093");

        let s = &copy_of(&out, suggest)[0];
        assert!(s.client.is_none());
        assert_eq!(s.suggestion.as_ref().unwrap().1.id, "1042");
        assert_eq!(row(&out, suggest).status, RowStatus::NeedsClient);

        let n = &copy_of(&out, none)[0];
        assert!(n.client.is_none() && n.suggestion.is_none());
        assert_eq!(n.looked_for.as_deref(), Some("Zzz Qqq"));
        assert_eq!(row(&out, none).status, RowStatus::NeedsClient);

        // Without the relationship: 0.98 resolves but is "not related"; 0.89 only suggests.
        let inp = copy_input(&f, &[related, far], &[]);
        let out = build_plan(&inp);
        let c = &copy_of(&out, far)[0];
        assert_eq!(c.client.as_ref().unwrap().id, "2093");
        assert!(matches!(
            c.client_match,
            CopyMatch::Name { related: false, .. }
        ));
        let c = &copy_of(&out, related)[0];
        assert!(c.client.is_none() && c.suggestion.is_some());
    }

    #[test]
    fn related_capture_manual_and_edits() {
        let f = fixture();
        let pkg = "Entity Co_1234_2025 K-1 package.pdf";
        let mut inp = copy_input(
            &f,
            &[pkg],
            &[
                ("1234", "2093", "shareholder"),
                ("1234", "1042", "partner"),
                ("1234", "4471", "member"),
                ("1234", "4472", "spouse"), // not a K-1 role
            ],
        );
        let out = build_plan(&inp);
        let ids: Vec<&str> = copy_of(&out, pkg)
            .iter()
            .map(|c| c.client.as_ref().unwrap().id.as_str())
            .collect();
        assert_eq!(ids, ["2093", "1042", "4471"], "by role");
        assert_eq!(summarize(&out.proposals).related_copies, 3);

        // × on one copy leaves it out; "+ Copy to…" adds one.
        let key = copy_of(&out, pkg)[1].key.clone();
        inp.edits.insert(
            f.inbox.join(pkg),
            RowEdits {
                removed_copies: [key].into(),
                added_copies: vec!["4472".into()],
                ..Default::default()
            },
        );
        let out = build_plan(&inp);
        let ids: Vec<&str> = copy_of(&out, pkg)
            .iter()
            .map(|c| c.client.as_ref().unwrap().id.as_str())
            .collect();
        assert_eq!(ids, ["2093", "4471", "4472"]);

        // No related clients: primary only, with a note.
        let mut inp = copy_input(&f, &[pkg], &[]);
        let out = build_plan(&inp);
        assert!(copy_of(&out, pkg).is_empty());
        assert_eq!(row(&out, pkg).copy_notes, ["no related clients"]);
        assert_eq!(row(&out, pkg).status, RowStatus::Ready);

        // Capture and Manual resolvers.
        let mut rule = Rule::new(
            "Shared 1099",
            r"1099 for (?P<id2>\d{4,})",
            "{Client}/Source Documents/{Year}",
            "{Year} 1099 - {Client}.{Ext}",
            true,
        );
        rule.copies.push(CopyDest::new(Resolver::Capture {
            group: "id2".into(),
        }));
        rule.copies.push(CopyDest::new(Resolver::Manual));
        inp.rules = vec![rule, Rule::catch_all()];
        let file = "Entity Co_1234_2025 1099 for 1042.pdf";
        inp.files = input(&f, &[file]).files;
        let out = build_plan(&inp);
        let copies = copy_of(&out, file);
        assert_eq!(copies[0].client.as_ref().unwrap().id, "1042");
        assert!(copies[1].client.is_none(), "manual copies wait for a pick");
        assert_eq!(row(&out, file).status, RowStatus::NeedsClient);
        // Picking the manual copy's client makes the row ready.
        inp.edits.insert(
            f.inbox.join(file),
            RowEdits {
                copy_clients: [(copies[1].key.clone(), "4471".to_string())].into(),
                ..Default::default()
            },
        );
        assert_eq!(build_plan(&inp).proposals[0].status, RowStatus::Ready);
    }

    #[test]
    fn close_name_match_resolves_without_a_click() {
        let f = fixture();
        // No usable ID; the name matches "Smith, John" at or above fuzzy_auto.
        let mut inp = input(&f, &["Smith John_9999_2025_1040.pdf"]);
        bind(
            &f,
            &mut inp,
            "Smith, John (1001)",
            "1001",
            FolderStatus::Bound,
        );
        let p = &build_plan(&inp).proposals[0];
        assert!(
            matches!(p.client_match, ClientMatch::Fuzzy(s) if s >= 0.97),
            "{:?}",
            p.client_match
        );
        assert_eq!(p.client.as_ref().unwrap().id, "1001");
        assert!(p.suggestion.is_none());
        assert_eq!(p.status, RowStatus::Ready);
    }

    #[test]
    fn duplicate_folders_pause_the_client() {
        let f = fixture();
        let mut inp = input(&f, &["Smith, John_1001_2025_1040.pdf"]);
        bind(&f, &mut inp, "Smith A", "1001", FolderStatus::Duplicate);
        bind(&f, &mut inp, "Smith B", "1001", FolderStatus::Duplicate);
        let out = build_plan(&inp);
        let p = &out.proposals[0];
        assert_eq!(p.status, RowStatus::Paused);
        assert!(!p.status.blocks_commit(), "left out, not blocking (X119)");
        let s = summarize(&out.proposals);
        assert_eq!((s.files, s.paused), (0, 1));
    }

    #[test]
    fn needs_client_with_suggestions_until_chosen() {
        let f = fixture();
        let mut inp = input(&f, &["Jonez Mary_9999_2025_1040.pdf", "scan0001.pdf"]);
        let out = build_plan(&inp);
        let p = row(&out, "Jonez Mary_9999_2025_1040.pdf");
        assert_eq!(p.status, RowStatus::NeedsClient);
        let (score, c) = p.suggestion.as_ref().unwrap();
        assert_eq!(c.id, "1003");
        assert!(*score >= 0.9);
        let scan = row(&out, "scan0001.pdf");
        assert_eq!(scan.status, RowStatus::NeedsClient);
        assert!(
            scan.suggestion.is_none(),
            "below the threshold: no one-click suggestion"
        );

        inp.edits.insert(
            f.inbox.join("Jonez Mary_9999_2025_1040.pdf"),
            RowEdits {
                client_id: Some("1003".into()),
                ..Default::default()
            },
        );
        let out = build_plan(&inp);
        let p = row(&out, "Jonez Mary_9999_2025_1040.pdf");
        assert_eq!(p.client_match, ClientMatch::Manual);
        assert_eq!(p.status, RowStatus::Ready);
        assert!(matches!(p.folder, Some(FolderAction::WillCreate(_))));
    }

    #[test]
    fn batch_year_and_row_year() {
        let f = fixture();
        let mut inp = input(
            &f,
            &["Acme LLC_1002_misc scan.pdf", "Acme LLC_1002_letter.pdf"],
        );
        let out = build_plan(&inp);
        assert!(
            out.proposals
                .iter()
                .all(|p| p.status == RowStatus::NeedsYear)
        );
        inp.batch.year = Some(2024);
        let out = build_plan(&inp);
        let p = row(&out, "Acme LLC_1002_misc scan.pdf");
        assert_eq!(p.status, RowStatus::Ready);
        assert_eq!((p.year, p.year_source), (Some(2024), YearSource::Batch));
        inp.edits.insert(
            f.inbox.join("Acme LLC_1002_letter.pdf"),
            RowEdits {
                year: Some(2023),
                ..Default::default()
            },
        );
        let out = build_plan(&inp);
        let p = row(&out, "Acme LLC_1002_letter.pdf");
        assert_eq!((p.year, p.year_source), (Some(2023), YearSource::Row));
    }

    #[test]
    fn folder_bind_create_and_conflict() {
        let f = fixture();
        let mut inp = input(
            &f,
            &[
                "Acme LLC_1002_2025_1040.pdf",
                "Baker Farms_1004_2025_1040.pdf",
                "Smith, John_1001_2025_1040.pdf",
            ],
        );
        let named = bind(
            &f,
            &mut inp,
            "Acme LLC (1002)",
            "1002",
            FolderStatus::UnboundNamed,
        );
        // A folder with the name the app would create, but not bound to Smith.
        std::fs::create_dir_all(f.root.join("Smith, John (1001)")).unwrap();
        let out = build_plan(&inp);
        assert_eq!(
            row(&out, "Acme LLC_1002_2025_1040.pdf").folder,
            Some(FolderAction::WillBind(named))
        );
        assert_eq!(
            row(&out, "Baker Farms_1004_2025_1040.pdf").folder,
            Some(FolderAction::WillCreate(f.root.join("Baker Farms (1004)")))
        );
        assert!(
            matches!(row(&out, "Smith, John_1001_2025_1040.pdf").status, RowStatus::Error(ref e) if e.contains("Conflicts"))
        );
    }

    #[test]
    fn missing_folder_waits_for_rescan_once() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_2025_1040.pdf"]);
        inp.rescan = Rescan::NotYet;
        let out = build_plan(&inp);
        assert!(out.needs_rescan);
        assert!(matches!(out.proposals[0].status, RowStatus::Pending(_)));
        inp.rescan = Rescan::Done;
        let out = build_plan(&inp);
        assert!(!out.needs_rescan);
        assert_eq!(out.proposals[0].status, RowStatus::Ready);
    }

    #[test]
    fn collisions_on_disk_and_in_batch() {
        let f = fixture();
        let mut inp = input(
            &f,
            &[
                "Acme LLC_1002_2025_1040.pdf",
                "Acme LLC_1002_2025 1040.pdf",
                "Acme LLC_1002_2025-1040.pdf",
            ],
        );
        let folder = bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let dir = folder.join("Tax Returns").join("2025");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2025 Tax Return - Acme LLC.pdf"), b"old").unwrap();

        // Default Keep both: (2), (3), (4); in-batch duplicates are always numbered.
        let out = build_plan(&inp);
        let mut names: Vec<String> = out
            .proposals
            .iter()
            .map(|p| {
                p.dest
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "2025 Tax Return - Acme LLC (2).pdf",
                "2025 Tax Return - Acme LLC (3).pdf",
                "2025 Tax Return - Acme LLC (4).pdf",
            ]
        );
        assert!(
            out.proposals
                .iter()
                .all(|p| p.collision && p.renamed && p.dest_dir_exists)
        );

        // Overwrite as batch default: the first overwrites, later ones in the batch are numbered.
        inp.batch.collision = CollisionChoice::Overwrite;
        let out = build_plan(&inp);
        let first = &out.proposals[0];
        assert_eq!(first.collision_choice, CollisionChoice::Overwrite);
        assert_eq!(
            first.dest.as_ref().unwrap().file_name().unwrap(),
            "2025 Tax Return - Acme LLC.pdf"
        );
        assert!(out.proposals[1].renamed);
        assert_eq!(summarize(&out.proposals).overwrites, 1);

        // Skip on one row.
        inp.edits.insert(
            inp.files[0].path.clone(),
            RowEdits {
                collision: Some(CollisionChoice::Skip),
                ..Default::default()
            },
        );
        let out = build_plan(&inp);
        assert_eq!(out.proposals[0].status, RowStatus::Skipped);
    }

    #[test]
    fn rename_keeps_the_extension_and_folder_override() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_organizer.pdf"]);
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let path = inp.files[0].path.clone();
        let plan = |inp: &PlanInput| build_plan(inp).proposals.remove(0);
        assert_eq!(plan(&inp).status, RowStatus::NeedsYear);
        inp.edits.insert(
            path.clone(),
            RowEdits {
                dest_rel: Some(PathBuf::from("Permanent")),
                ..Default::default()
            },
        );
        assert_eq!(
            plan(&inp).status,
            RowStatus::NeedsYear,
            "the Organizer's name uses {{Year}}"
        );
        // V6: a picked folder with a typed name needs no year.
        inp.edits.insert(
            path.clone(),
            RowEdits {
                dest_rel: Some(PathBuf::from("Permanent").join("Setup")),
                filename: Some("Articles.txt".into()),
                ..Default::default()
            },
        );
        let p = plan(&inp);
        assert_eq!(p.status, RowStatus::Ready);
        assert_eq!(
            p.dest.unwrap(),
            f.root
                .join("Acme (1002)")
                .join("Permanent")
                .join("Setup")
                .join("Articles.txt.pdf"),
            "V5: the file keeps its extension"
        );
        for bad in ["..", "a/../../b", "CON", "C:/x"] {
            inp.edits.get_mut(&path).unwrap().filename = Some("x".into());
            inp.edits.get_mut(&path).unwrap().dest_rel = Some(PathBuf::from(bad));
            assert!(
                matches!(plan(&inp).status, RowStatus::Error(_)),
                "{bad} refused"
            );
        }
    }

    /// V4: a collision records what is there: another row of the batch, or a file on disk.
    #[test]
    fn collision_says_what_is_there() {
        let f = fixture();
        let names = [
            "Acme LLC_1002_2025_1040.pdf",
            "Acme LLC_1002_2025 return.pdf",
        ];
        let mut inp = input(&f, &names);
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let out = build_plan(&inp);
        let (a, b) = (&out.proposals[0], &out.proposals[1]);
        assert!(!a.collision);
        match &b.collision_with {
            Some(CollisionWith::Batch { row, path }) => {
                assert_eq!(row, &a.file.path);
                assert_eq!(Some(path), a.dest.as_ref());
            }
            other => panic!("{other:?}"),
        }
        let existing = a.dest.clone().unwrap();
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, b"old").unwrap();
        let out = build_plan(&inp);
        match &out.proposals[0].collision_with {
            Some(CollisionWith::Disk { path, bytes, .. }) => {
                assert_eq!(path, &existing);
                assert_eq!(*bytes, Some(3));
            }
            other => panic!("{other:?}"),
        }
    }

    /// V3: a ticked row claims its name first, so committing only it isn't numbered " (2)"
    /// because of an unticked row; the proposals keep the queue order.
    #[test]
    fn ticked_rows_are_numbered_first() {
        let f = fixture();
        let names = [
            "Acme LLC_1002_2025_1040.pdf",
            "Acme LLC_1002_2025 return.pdf",
        ];
        let mut inp = input(&f, &names);
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let file = |out: &PlanOutput, i: usize| {
            out.proposals[i]
                .dest
                .as_ref()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        let out = build_plan(&inp);
        assert_eq!(file(&out, 0), "2025 Tax Return - Acme LLC.pdf");
        assert_eq!(file(&out, 1), "2025 Tax Return - Acme LLC (2).pdf");
        inp.batch.ticked = vec![inp.files[1].path.clone()];
        let out = build_plan(&inp);
        assert!(
            out.proposals[0].file.path.ends_with(names[0]),
            "queue order kept"
        );
        assert_eq!(file(&out, 1), "2025 Tax Return - Acme LLC.pdf");
        assert!(!out.proposals[1].collision);
        assert_eq!(file(&out, 0), "2025 Tax Return - Acme LLC (2).pdf");
    }

    /// Firm rules review: text after "K-1" that names no client files the K-1 without a copy
    /// instead of asking for a client.
    #[test]
    fn k1_without_a_client_name_files_without_a_copy() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_2025 K-1 summary all partners.pdf"]);
        inp.rules = rules::firm_rules();
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let p = build_plan(&inp).proposals.remove(0);
        assert_eq!(p.rule_name, "K-1");
        assert_eq!(p.status, RowStatus::Ready, "{:?}", p.copy_notes);
        assert_eq!(p.copies.len(), 1);
        assert_eq!(p.copies[0].status, CopyStatus::Skipped);
        assert!(p.copy_notes.iter().any(|n| n.contains("is not a client")));
        assert_eq!(
            p.dest.unwrap().file_name().unwrap(),
            "2025 K-1 - summary all partners.pdf"
        );
    }

    #[test]
    fn keep_original_name_and_skip_row() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_2025_1040.pdf"]);
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let path = inp.files[0].path.clone();
        inp.edits.insert(
            path.clone(),
            RowEdits {
                keep_original_name: true,
                ..Default::default()
            },
        );
        let out = build_plan(&inp);
        assert_eq!(
            out.proposals[0].dest.as_ref().unwrap().file_name().unwrap(),
            "Acme LLC_1002_2025_1040.pdf"
        );
        inp.edits.insert(
            path,
            RowEdits {
                skip: true,
                ..Default::default()
            },
        );
        assert_eq!(build_plan(&inp).proposals[0].status, RowStatus::Skipped);
    }

    #[test]
    fn typed_name_gets_extension_and_original_name_wins() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_2025_1040.pdf"]);
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let path = inp.files[0].path.clone();
        inp.edits.insert(
            path.clone(),
            RowEdits {
                filename: Some("Packet: 2025".into()),
                ..Default::default()
            },
        );
        let name = |inp: &PlanInput| {
            build_plan(inp).proposals[0]
                .dest
                .as_ref()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(name(&inp), "Packet_ 2025.pdf", "sanitized, extension added");
        inp.edits.get_mut(&path).unwrap().keep_original_name = true;
        assert_eq!(name(&inp), "Acme LLC_1002_2025_1040.pdf");
    }

    #[test]
    fn partly_filed_split_original_is_an_error_row() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_2025 print run.pdf"]);
        inp.partial = vec![inp.files[0].path.clone()];
        assert!(
            matches!(&build_plan(&inp).proposals[0].status, RowStatus::Error(e) if e.contains("already filed"))
        );
    }

    #[test]
    fn page_text_sniff_and_rule_override() {
        let f = fixture();
        let mut inp = input(&f, &["Acme LLC_1002_2025 scan.pdf"]);
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        let path = inp.files[0].path.clone();
        inp.sniff_enabled = true;
        let out = build_plan(&inp);
        assert_eq!(out.needs_sniff, vec![path.clone()]);
        assert!(matches!(out.proposals[0].status, RowStatus::Pending(_)));

        inp.sniff.insert(
            path.clone(),
            PageText::Text("ENGAGEMENT LETTER\nDear client".into()),
        );
        let out = build_plan(&inp);
        let p = &out.proposals[0];
        assert_eq!(
            (p.rule_name.as_str(), p.rule_source),
            ("Engagement Letter", RuleSource::PageText)
        );
        assert_eq!(p.status, RowStatus::Ready);

        inp.sniff
            .insert(path.clone(), PageText::Failed("scan".into()));
        assert_eq!(
            build_plan(&inp).proposals[0].rule_source,
            RuleSource::CatchAll
        );

        let organizer = inp
            .rules
            .iter()
            .find(|r| r.name == "Organizer")
            .unwrap()
            .id
            .clone();
        inp.edits.insert(
            path,
            RowEdits {
                rule_id: Some(organizer),
                ..Default::default()
            },
        );
        let p = &build_plan(&inp).proposals[0];
        assert_eq!(
            (p.rule_name.as_str(), p.rule_source),
            ("Organizer", RuleSource::Override)
        );
    }

    #[test]
    fn summary_counts() {
        let f = fixture();
        let mut inp = input(
            &f,
            &[
                "Acme LLC_1002_2025_1040.pdf",
                "Baker Farms_1004_2025_1040.pdf",
                "Baker Farms_1004_2025 organizer.pdf",
                "nobody.pdf",
            ],
        );
        bind(&f, &mut inp, "Acme (1002)", "1002", FolderStatus::Bound);
        inp.edits.insert(
            inp.files[3].path.clone(),
            RowEdits {
                skip: true,
                ..Default::default()
            },
        );
        let s = summarize(&build_plan(&inp).proposals);
        assert_eq!(
            s,
            Summary {
                files: 3,
                moves: 3,
                copies: 0,
                new_client_folders: 1,
                binds: 0,
                new_sub_folders: 3,
                overwrites: 0,
                renamed: 0,
                skipped: 1,
                paused: 0,
                related_copies: 0,
            }
        );
    }

    #[test]
    fn reserved_client_name_is_an_error_row() {
        let f = fixture();
        let mut inp = input(&f, &["Smith, John_1001_2025_1040.pdf"]);
        inp.template = TemplateNode::new("{Client}");
        let mut list = (*f.clients).clone();
        list.clients[0].name = "CON".into();
        inp.clients = Some(Arc::new(list));
        assert!(matches!(
            build_plan(&inp).proposals[0].status,
            RowStatus::Error(_)
        ));
    }
}
