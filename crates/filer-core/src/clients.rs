//! Client master: CSV import with a column map, the `data/clients.json` cache, and matching.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{ColumnMap, Level};
use crate::template::BUILTIN_TOKENS;
use crate::worker;

pub const CLIENTS_FILE: &str = "clients.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Client {
    pub id: String,
    /// Stable per CSV ID (see [`ClientList::assign_uuids`]); the sentinel's `client_id`.
    #[serde(default)]
    pub uuid: Uuid,
    pub name: String,
    /// Extra CSV columns, keyed by token name (see [`token_name`]).
    pub extra: BTreeMap<String, String>,
}

/// The imported master list as cached in `clients.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientList {
    pub source: PathBuf,
    /// CSV modification time at import; a different time on launch triggers a re-import.
    pub source_modified: DateTime<Utc>,
    pub imported_at: DateTime<Utc>,
    pub column_map: ColumnMap,
    /// Rows dropped because the ID or name was empty.
    pub skipped_rows: usize,
    pub clients: Vec<Client>,
    /// Every CSV ID ever imported and its uuid, so an ID that leaves the CSV and comes back
    /// keeps its uuid.
    #[serde(default)]
    pub uuids: BTreeMap<String, Uuid>,
    #[serde(skip)]
    index: Index,
}

#[derive(Debug, Clone, Default)]
struct Index {
    by_id: HashMap<String, usize>,
    /// Normalized name variants per client, same order as `clients`.
    names: Vec<Vec<String>>,
}

impl ClientList {
    fn new(
        source: PathBuf,
        source_modified: DateTime<Utc>,
        column_map: ColumnMap,
        skipped_rows: usize,
        clients: Vec<Client>,
    ) -> Self {
        let mut list = Self {
            source,
            source_modified,
            imported_at: Utc::now(),
            column_map,
            skipped_rows,
            clients,
            uuids: BTreeMap::new(),
            index: Index::default(),
        };
        list.assign_uuids(&BTreeMap::new());
        list.build_index();
        list
    }

    /// Gives each client the uuid recorded for its ID (in `known`, then in this list), or a new
    /// one. Returns `true` if any client got a new uuid.
    pub fn assign_uuids(&mut self, known: &BTreeMap<String, Uuid>) -> bool {
        for (id, u) in known {
            self.uuids.insert(id.clone(), *u);
        }
        let mut fresh = false;
        for c in &mut self.clients {
            c.uuid = *self.uuids.entry(c.id.clone()).or_insert_with(|| {
                fresh = true;
                Uuid::new_v4()
            });
        }
        fresh
    }

    fn build_index(&mut self) {
        self.index.by_id = self
            .clients
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        self.index.names = self
            .clients
            .iter()
            .map(|c| name_variants(&c.name))
            .collect();
    }

    /// Import time in local time, for display.
    pub fn imported_text(&self) -> String {
        self.imported_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    pub fn len(&self) -> usize {
        self.clients.len()
    }

    /// Exact ID match (after trimming). Leading zeros are significant.
    pub fn match_by_id(&self, id: &str) -> Option<&Client> {
        self.index.by_id.get(id.trim()).map(|&i| &self.clients[i])
    }

    /// Some client's name is at least `threshold` similar to `query` (as [`Self::fuzzy_by_name`]
    /// scores it); stops at the first one.
    pub fn any_name_at_least(&self, query: &str, threshold: f64) -> bool {
        let query = name_variants(query);
        self.index.names.iter().any(|names| {
            query.iter().any(|q| {
                names
                    .iter()
                    .any(|n| strsim::jaro_winkler(q, n) >= threshold)
            })
        })
    }

    /// The `limit` best clients by Jaro-Winkler similarity of normalized names, best first.
    /// "Last, First" and "First Last" forms are compared both ways.
    pub fn fuzzy_by_name(&self, query: &str, limit: usize) -> Vec<(f64, &Client)> {
        let query = name_variants(query);
        if query.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(f64, &Client)> = self
            .clients
            .iter()
            .zip(&self.index.names)
            .map(|(c, names)| {
                let best = query
                    .iter()
                    .flat_map(|q| names.iter().map(move |n| strsim::jaro_winkler(q, n)))
                    .fold(0.0, f64::max);
                (best, c)
            })
            .collect();
        let order = |a: &(f64, &Client), b: &(f64, &Client)| {
            b.0.total_cmp(&a.0)
                .then_with(|| a.1.name.cmp(&b.1.name))
                .then_with(|| a.1.id.cmp(&b.1.id))
        };
        if scored.len() > limit {
            scored.select_nth_unstable_by(limit - 1, order);
            scored.truncate(limit);
        }
        scored.sort_by(order);
        scored
    }
}

/// Jaro-Winkler similarity of two names after normalizing, trying the "Last, First" swap both
/// ways (as [`ClientList::fuzzy_by_name`] does).
pub fn name_score(query: &str, name: &str) -> f64 {
    let q = name_variants(query);
    let n = name_variants(name);
    q.iter()
        .flat_map(|a| n.iter().map(move |b| strsim::jaro_winkler(a, b)))
        .fold(0.0, f64::max)
}

/// Lowercase, apostrophes and periods dropped, other punctuation to spaces, whitespace collapsed.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if matches!(ch, '\'' | '\u{2019}' | '.') {
            continue;
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// The normalized name, plus "First Last" when the name is written "Last, First".
fn name_variants(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let plain = normalize_name(name);
    if !plain.is_empty() {
        out.push(plain);
    }
    if let Some((last, first)) = name.split_once(',') {
        let swapped = normalize_name(&format!("{first} {last}"));
        if !swapped.is_empty() && !out.contains(&swapped) {
            out.push(swapped);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// CSV reading

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the file is UTF-16 text; save it from Excel as \"CSV UTF-8\" or plain CSV")]
    Utf16,
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("the CSV has no header row")]
    NoHeader,
    #[error("{}", .0.join(" "))]
    Mapping(Vec<String>),
    #[error("duplicate client IDs: {}", list_ids(.0))]
    DuplicateIds(Vec<String>),
    #[error("cannot write {path}: {source}")]
    Cache {
        path: PathBuf,
        source: anyhow::Error,
    },
}

fn list_ids(ids: &[String]) -> String {
    const SHOW: usize = 20;
    let shown = ids
        .iter()
        .take(SHOW)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if ids.len() > SHOW {
        format!("{shown} and {} more", ids.len() - SHOW)
    } else {
        shown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Windows1252,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 (BOM)",
            Encoding::Windows1252 => "Windows-1252",
        }
    }
}

/// A whole CSV file in memory: header row plus data rows (as written, not trimmed).
#[derive(Debug, Clone)]
pub struct CsvTable {
    pub path: PathBuf,
    pub modified: DateTime<Utc>,
    pub encoding: Encoding,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn read_csv(path: &Path) -> Result<CsvTable, ImportError> {
    let io = |source| ImportError::Io {
        path: path.to_path_buf(),
        source,
    };
    let modified = file_modified(path).map_err(io)?;
    let bytes = std::fs::read(path).map_err(io)?;
    let (text, encoding) = decode(bytes)?;

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    if headers.iter().all(String::is_empty) {
        return Err(ImportError::NoHeader);
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        rows.push(record?.iter().map(str::to_string).collect());
    }
    Ok(CsvTable {
        path: path.to_path_buf(),
        modified,
        encoding,
        headers,
        rows,
    })
}

fn file_modified(path: &Path) -> std::io::Result<DateTime<Utc>> {
    Ok(std::fs::metadata(path)?.modified()?.into())
}

/// UTF-8 (with or without BOM); otherwise Windows-1252. UTF-16 is rejected. A UTF-8 BOM is
/// dropped even when the rest falls back to Windows-1252.
pub fn decode(mut bytes: Vec<u8>) -> Result<(String, Encoding), ImportError> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(ImportError::Utf16);
    }
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    if bom {
        bytes.drain(..3);
    }
    match String::from_utf8(bytes) {
        Ok(s) => Ok((
            s,
            if bom {
                Encoding::Utf8Bom
            } else {
                Encoding::Utf8
            },
        )),
        Err(e) => Ok((
            e.as_bytes().iter().map(|&b| windows_1252(b)).collect(),
            Encoding::Windows1252,
        )),
    }
}

/// Windows-1252 byte to char. Bytes 0x80–0x9F use the code page's table (the five unassigned
/// ones map to the matching C1 control, as browsers do); every other byte equals its Latin-1 char.
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

// ---------------------------------------------------------------------------
// Mapping

/// Template token for a CSV header: ASCII letters, digits and `_`; any other run of characters
/// becomes one `_`; leading/trailing `_` trimmed. "Client Type" → `Client_Type`.
pub fn token_name(header: &str) -> String {
    let mut out = String::new();
    for ch in header.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// Problems with a column map for these headers. Empty means it can be imported.
pub fn validate_mapping(headers: &[String], map: &ColumnMap) -> Vec<String> {
    let mut errors = Vec::new();
    let count = |h: &str| headers.iter().filter(|x| *x == h).count();
    let check_column = |h: &str, errors: &mut Vec<String>| match count(h) {
        0 => errors.push(format!("Column \"{h}\" is not in this CSV.")),
        1 => {}
        _ => errors.push(format!(
            "Column \"{h}\" appears more than once in the header row."
        )),
    };

    match &map.id {
        None => errors.push("Choose the ID column.".into()),
        Some(h) => check_column(h, &mut errors),
    }
    match &map.name {
        None => errors.push("Choose the name column.".into()),
        Some(h) => check_column(h, &mut errors),
    }
    if map.id.is_some() && map.id == map.name {
        errors.push("The ID and name must be different columns.".into());
    }

    let mut tokens: Vec<(String, &str)> = Vec::new();
    for h in extras_in_use(map) {
        check_column(h, &mut errors);
        let token = token_name(h);
        if token.is_empty() {
            errors.push(format!(
                "Column \"{h}\" has no letters or digits to use as a token name."
            ));
            continue;
        }
        if let Some(b) = BUILTIN_TOKENS
            .iter()
            .find(|b| b.eq_ignore_ascii_case(&token))
        {
            errors.push(format!(
                "Column \"{h}\" would become {{{token}}}, which clashes with the built-in {{{b}}}."
            ));
            continue;
        }
        if let Some((_, other)) = tokens.iter().find(|(t, _)| t.eq_ignore_ascii_case(&token)) {
            errors.push(format!(
                "Columns \"{other}\" and \"{h}\" would both become {{{token}}}."
            ));
            continue;
        }
        tokens.push((token, h));
    }
    errors
}

/// Template tokens the column map's extra columns provide.
pub fn extra_tokens(map: &ColumnMap) -> Vec<String> {
    extras_in_use(map)
        .map(|h| token_name(h))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Extra columns, minus any that are also the ID or name column.
fn extras_in_use(map: &ColumnMap) -> impl Iterator<Item = &String> {
    map.extras
        .iter()
        .filter(move |h| Some(*h) != map.id.as_ref() && Some(*h) != map.name.as_ref())
}

/// Builds the client list. Rows with an empty ID or name are skipped and counted; duplicate IDs
/// fail the whole import.
pub fn apply_mapping(table: &CsvTable, map: &ColumnMap) -> Result<ClientList, ImportError> {
    let errors = validate_mapping(&table.headers, map);
    if !errors.is_empty() {
        return Err(ImportError::Mapping(errors));
    }
    let col = |h: &String| {
        table
            .headers
            .iter()
            .position(|x| x == h)
            .expect("validated")
    };
    let id_col = col(map.id.as_ref().expect("validated"));
    let name_col = col(map.name.as_ref().expect("validated"));
    let extras: Vec<(String, usize)> = extras_in_use(map)
        .map(|h| (token_name(h), col(h)))
        .collect();
    let cell = |row: &[String], i: usize| row.get(i).map_or("", |s| s.trim()).to_string();

    let mut clients = Vec::with_capacity(table.rows.len());
    let mut skipped = 0;
    let mut seen: HashMap<String, usize> = HashMap::new();
    for row in &table.rows {
        let id = cell(row, id_col);
        let name = cell(row, name_col);
        if id.is_empty() || name.is_empty() {
            skipped += 1;
            continue;
        }
        *seen.entry(id.clone()).or_default() += 1;
        let extra = extras
            .iter()
            .map(|(token, i)| (token.clone(), cell(row, *i)))
            .collect();
        clients.push(Client {
            id,
            uuid: Uuid::nil(),
            name,
            extra,
        });
    }

    let mut dups: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(id, _)| id)
        .collect();
    if !dups.is_empty() {
        dups.sort();
        return Err(ImportError::DuplicateIds(dups));
    }
    Ok(ClientList::new(
        table.path.clone(),
        table.modified,
        map.clone(),
        skipped,
        clients,
    ))
}

// ---------------------------------------------------------------------------
// Cache

pub fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CLIENTS_FILE)
}

/// `Ok(None)` when there is no cache yet.
pub fn load_cache(data_dir: &Path) -> anyhow::Result<Option<ClientList>> {
    let path = cache_path(data_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut list: ClientList = serde_json::from_str(&text)?;
    // A cache from v0.1.0 has no uuids yet (X115): assign them once and keep them.
    if list.assign_uuids(&BTreeMap::new())
        && let Err(e) = save_cache(data_dir, &list)
    {
        tracing::error!(error = %e, "could not save new client uuids to clients.json");
    }
    list.build_index();
    Ok(Some(list))
}

/// The uuids recorded in the current cache, so a re-import keeps them.
fn known_uuids(data_dir: &Path) -> BTreeMap<String, Uuid> {
    match load_cache(data_dir) {
        Ok(Some(c)) => c.uuids,
        Ok(None) => BTreeMap::new(),
        Err(e) => {
            tracing::warn!(error = %e, "clients.json unreadable; new client uuids will be made");
            BTreeMap::new()
        }
    }
}

/// Compact JSON (the list can be large), written atomically like the other data files.
pub fn save_cache(data_dir: &Path, list: &ClientList) -> anyhow::Result<()> {
    crate::datafile::write_atomic(&cache_path(data_dir), &serde_json::to_vec(list)?)
}

fn import_and_cache(
    data_dir: &Path,
    csv: &Path,
    map: &ColumnMap,
) -> Result<ClientList, ImportError> {
    let mut list = apply_mapping(&read_csv(csv)?, map)?;
    list.assign_uuids(&known_uuids(data_dir));
    save_cache(data_dir, &list).map_err(|source| ImportError::Cache {
        path: cache_path(data_dir),
        source,
    })?;
    Ok(list)
}

/// Result of loading the client list: the list to use (if any) and a message for Setup.
#[derive(Debug)]
pub struct Loaded {
    pub list: Option<ClientList>,
    pub note: Option<(Level, String)>,
}

/// Launch behaviour: load the cache, and re-import silently if the CSV changed since (different
/// path or modification time) or the saved column map differs from the cached one. A failed
/// re-import keeps the cached list and reports why.
pub fn load_on_launch(data_dir: &Path, csv: Option<&Path>, map: &ColumnMap) -> Loaded {
    let cached = match load_cache(data_dir) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "clients.json unreadable; ignoring it");
            None
        }
    };
    let Some(csv) = csv else {
        return Loaded {
            list: cached,
            note: None,
        };
    };
    let stale = match (&cached, file_modified(csv)) {
        (Some(c), Ok(m)) => c.source != csv || c.source_modified != m || c.column_map != *map,
        _ => true,
    };
    if !stale {
        return Loaded {
            list: cached,
            note: None,
        };
    }
    match import_and_cache(data_dir, csv, map) {
        Ok(list) => {
            tracing::info!(clients = list.len(), csv = %csv.display(), "client list re-imported");
            Loaded {
                list: Some(list),
                note: None,
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "client list re-import failed");
            let note = match &cached {
                Some(c) => format!(
                    "The client CSV changed but could not be re-imported: {e}. Using the list imported {}.",
                    c.imported_text()
                ),
                None => format!("The client CSV could not be imported: {e}."),
            };
            Loaded {
                list: cached,
                note: Some((Level::Error, note)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Background worker

/// What started a job, so its result goes to the right place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Launch,
    Refresh,
    Wizard,
}

enum Done {
    Loaded(Loaded),
    Imported(Result<ClientList, String>),
}

struct Job {
    /// Start order. Results are applied oldest first, and a result older than the list already
    /// shown never replaces it.
    seq: u64,
    origin: Origin,
    rx: Receiver<Done>,
}

/// Holds the current client list and runs load/refresh/import jobs off the UI thread.
pub struct ClientsManager {
    list: Option<Arc<ClientList>>,
    note: Option<(Level, String)>,
    jobs: Vec<Job>,
    next_seq: u64,
    /// `seq` of the job whose list is shown (0 = none yet).
    shown_seq: u64,
    wizard_result: Option<Result<(), String>>,
}

/// Returned by [`ClientsManager::poll`] when an import was saved to `clients.json`, so the caller
/// can store the CSV path and column map in the settings.
#[derive(Debug)]
pub struct Committed {
    pub source: PathBuf,
    pub column_map: ColumnMap,
}

impl ClientsManager {
    pub fn new() -> Self {
        Self {
            list: None,
            note: None,
            jobs: Vec::new(),
            next_seq: 1,
            shown_seq: 0,
            wizard_result: None,
        }
    }

    pub fn list(&self) -> Option<&Arc<ClientList>> {
        self.list.as_ref()
    }

    pub fn note(&self) -> Option<&(Level, String)> {
        self.note.as_ref()
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Launch load: uses the cache, re-importing only when the CSV or column map changed.
    pub fn load(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        csv: Option<PathBuf>,
        map: ColumnMap,
    ) {
        self.spawn(ctx, Origin::Launch, move || {
            Done::Loaded(load_on_launch(&data_dir, csv.as_deref(), &map))
        });
    }

    /// Refresh button: always re-imports with the saved column map.
    pub fn refresh(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        csv: PathBuf,
        map: ColumnMap,
    ) {
        self.spawn(ctx, Origin::Refresh, move || {
            Done::Imported(import_and_cache(&data_dir, &csv, &map).map_err(|e| e.to_string()))
        });
    }

    /// Wizard Import: builds the list from the table already read, and writes `clients.json`.
    /// The outcome is collected with [`Self::take_wizard_result`].
    pub fn import(
        &mut self,
        ctx: &egui::Context,
        data_dir: PathBuf,
        table: Arc<CsvTable>,
        map: ColumnMap,
    ) {
        self.wizard_result = None;
        self.spawn(ctx, Origin::Wizard, move || {
            let result = apply_mapping(&table, &map)
                .map_err(|e| e.to_string())
                .and_then(|mut list| {
                    list.assign_uuids(&known_uuids(&data_dir));
                    save_cache(&data_dir, &list)
                        .map(|()| list)
                        .map_err(|e| format!("cannot write clients.json: {e}"))
                });
            Done::Imported(result)
        });
    }

    /// `Ok` once the wizard's import is saved, `Err` with the reason if it failed.
    pub fn take_wizard_result(&mut self) -> Option<Result<(), String>> {
        self.wizard_result.take()
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        job: impl FnOnce() -> Done + Send + 'static,
    ) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.jobs.push(Job {
            seq,
            origin,
            rx: worker::background(ctx, job),
        });
    }

    /// Collects finished jobs. Call every frame.
    pub fn poll(&mut self) -> Option<Committed> {
        let mut finished: Vec<(u64, Origin, Option<Done>)> = Vec::new();
        self.jobs.retain(|job| match job.rx.try_recv() {
            Ok(done) => {
                finished.push((job.seq, job.origin, Some(done)));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                finished.push((job.seq, job.origin, None));
                false
            }
        });
        finished.sort_by_key(|f| f.0);

        let mut committed = None;
        for (seq, origin, done) in finished {
            match done {
                None => {
                    tracing::error!(?origin, "client job thread ended without a result");
                    self.report_failure(
                        origin,
                        "the background job stopped unexpectedly (see filer.log)".into(),
                    );
                }
                Some(Done::Loaded(loaded)) => {
                    if seq < self.shown_seq {
                        continue;
                    }
                    if let Some(list) = loaded.list {
                        self.list = Some(Arc::new(list));
                        self.shown_seq = seq;
                    }
                    self.note = loaded.note;
                }
                Some(Done::Imported(Ok(list))) => {
                    if origin == Origin::Wizard {
                        self.wizard_result = Some(Ok(()));
                    }
                    if seq < self.shown_seq {
                        continue;
                    }
                    self.note = Some((
                        Level::Info,
                        format!(
                            "Imported {} clients{}.",
                            list.len(),
                            skipped_text(list.skipped_rows)
                        ),
                    ));
                    committed = Some(Committed {
                        source: list.source.clone(),
                        column_map: list.column_map.clone(),
                    });
                    self.list = Some(Arc::new(list));
                    self.shown_seq = seq;
                }
                Some(Done::Imported(Err(e))) => self.report_failure(origin, e),
            }
        }
        committed
    }

    fn report_failure(&mut self, origin: Origin, msg: String) {
        if origin == Origin::Wizard {
            self.wizard_result = Some(Err(msg));
        } else {
            self.note = Some((Level::Error, format!("Import failed: {msg}")));
        }
    }
}

impl Default for ClientsManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn skipped_text(skipped: usize) -> String {
    match skipped {
        0 => String::new(),
        1 => " (1 row skipped: empty ID or name)".into(),
        n => format!(" ({n} rows skipped: empty ID or name)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime};

    fn map(id: &str, name: &str, extras: &[&str]) -> ColumnMap {
        ColumnMap {
            id: Some(id.into()),
            name: Some(name.into()),
            extras: extras.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn write_csv(dir: &Path, text: &str) -> PathBuf {
        let p = dir.join("clients.csv");
        std::fs::write(&p, text).unwrap();
        p
    }

    fn list_of(names: &[(&str, &str)]) -> ClientList {
        let clients = names
            .iter()
            .map(|(id, name)| Client {
                uuid: uuid::Uuid::new_v4(),
                id: id.to_string(),
                name: name.to_string(),
                extra: BTreeMap::new(),
            })
            .collect();
        ClientList::new(
            PathBuf::from("x.csv"),
            Utc::now(),
            ColumnMap::default(),
            0,
            clients,
        )
    }

    #[test]
    fn decodes_utf8_bom_and_windows_1252() {
        assert_eq!(
            decode(b"Jos\xC3\xA9".to_vec()).unwrap(),
            ("José".into(), Encoding::Utf8)
        );
        assert_eq!(
            decode(b"\xEF\xBB\xBFid".to_vec()).unwrap(),
            ("id".into(), Encoding::Utf8Bom)
        );
        assert_eq!(
            decode(b"Jos\xE9 O\x92Brien \x80".to_vec()).unwrap(),
            ("José O\u{2019}Brien \u{20AC}".into(), Encoding::Windows1252)
        );
        assert!(matches!(
            decode(b"\xFF\xFEi\0d\0".to_vec()),
            Err(ImportError::Utf16)
        ));
        // BOM, then a non-UTF-8 body: the BOM must not leak into the first header.
        assert_eq!(
            decode(b"\xEF\xBB\xBFClient ID,Name\nJos\xE9".to_vec()).unwrap(),
            ("Client ID,Name\nJosé".into(), Encoding::Windows1252)
        );
    }

    #[test]
    fn token_names() {
        assert_eq!(token_name("Client Type"), "Client_Type");
        assert_eq!(token_name("  E-mail / Fax "), "E_mail_Fax");
        assert_eq!(token_name("Préparer"), "Pr_parer");
        assert_eq!(token_name("snake_case"), "snake_case");
        assert_eq!(token_name("!!!"), "");
    }

    #[test]
    fn mapping_builds_clients_and_skips_empty_rows() {
        let dir = tempfile::tempdir().unwrap();
        let csv = write_csv(
            dir.path(),
            "Client ID,Client Name,Entity Type,Partner\n\
             1001,\" Smith, John \",1040,KW\n\
             1002,Acme LLC,1065\n\
             ,No Id,1040,KW\n\
             1003,,1120,KW\n",
        );
        let table = read_csv(&csv).unwrap();
        let list = apply_mapping(
            &table,
            &map("Client ID", "Client Name", &["Entity Type", "Partner"]),
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.skipped_rows, 2);
        let c = list.match_by_id("1001").unwrap();
        assert_eq!(c.name, "Smith, John");
        assert_eq!(c.extra["Entity_Type"], "1040");
        assert_eq!(c.extra["Partner"], "KW");
        // Short row: missing cells read as empty.
        assert_eq!(list.match_by_id("1002").unwrap().extra["Partner"], "");
    }

    #[test]
    fn duplicate_ids_fail_and_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let csv = write_csv(
            dir.path(),
            "id,name\n1001,A\n1002,B\n1001,C\n1003,D\n1002,E\n",
        );
        let err = apply_mapping(&read_csv(&csv).unwrap(), &map("id", "name", &[])).unwrap_err();
        match &err {
            ImportError::DuplicateIds(ids) => assert_eq!(ids, &["1001", "1002"]),
            other => panic!("{other:?}"),
        }
        assert_eq!(err.to_string(), "duplicate client IDs: 1001, 1002");
    }

    #[test]
    fn mapping_validation() {
        let headers: Vec<String> = ["id", "name", "Year", "Fee $", "Fee #", "dup", "dup", "%%"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(validate_mapping(&headers, &map("id", "name", &[])).is_empty());
        let none = validate_mapping(&headers, &ColumnMap::default());
        assert_eq!(none.len(), 2);
        let same = validate_mapping(&headers, &map("id", "id", &[]));
        assert!(same.iter().any(|e| e.contains("different columns")));

        let errs = validate_mapping(
            &headers,
            &map(
                "id",
                "name",
                &["Year", "Fee $", "Fee #", "dup", "%%", "gone"],
            ),
        );
        let has = |s: &str| errs.iter().any(|e| e.contains(s));
        assert!(has("built-in {Year}"), "{errs:?}");
        assert!(has("would both become {Fee}"), "{errs:?}");
        assert!(has("\"dup\" appears more than once"), "{errs:?}");
        assert!(has("\"%%\" has no letters"), "{errs:?}");
        assert!(has("\"gone\" is not in this CSV"), "{errs:?}");
        // The ID/name columns listed as extras are ignored, not errors.
        assert!(validate_mapping(&headers, &map("id", "name", &["id"])).is_empty());
    }

    #[test]
    fn utf16_and_headerless_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("u16.csv");
        std::fs::write(&p, b"\xFF\xFEi\0d\0").unwrap();
        assert!(matches!(read_csv(&p), Err(ImportError::Utf16)));
        let p = write_csv(dir.path(), "");
        assert!(matches!(read_csv(&p), Err(ImportError::NoHeader)));
    }

    #[test]
    fn windows_1252_csv_imports_accented_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.csv");
        std::fs::write(&p, b"id,name\n1001,Jos\xE9 Ni\xF1o\n").unwrap();
        let table = read_csv(&p).unwrap();
        assert_eq!(table.encoding, Encoding::Windows1252);
        let list = apply_mapping(&table, &map("id", "name", &[])).unwrap();
        assert_eq!(list.match_by_id("1001").unwrap().name, "José Niño");
    }

    #[test]
    fn id_match_is_exact_after_trim() {
        let list = list_of(&[("01234", "A"), ("1234", "B")]);
        assert_eq!(list.match_by_id(" 01234 ").unwrap().name, "A");
        assert_eq!(list.match_by_id("1234").unwrap().name, "B");
        assert!(list.match_by_id("234").is_none());
    }

    #[test]
    fn normalization() {
        assert_eq!(
            normalize_name("  O'Brien,  Mary-Kate  "),
            "obrien mary kate"
        );
        assert_eq!(normalize_name("SMITH & SONS, INC."), "smith sons inc");
        assert_eq!(normalize_name("José"), "josé");
    }

    #[test]
    fn fuzzy_ordering_and_name_swap() {
        let list = list_of(&[
            ("1001", "Smith, John"),
            ("1002", "Smyth, Joan"),
            ("1003", "Johnson Partners LLC"),
            ("1004", "Jones, Mary"),
        ]);
        let ids = |q: &str| -> Vec<String> {
            list.fuzzy_by_name(q, 10)
                .iter()
                .map(|(_, c)| c.id.clone())
                .collect()
        };

        // "First Last" query finds the "Last, First" record exactly.
        let top = &list.fuzzy_by_name("John Smith", 10)[0];
        assert_eq!(top.1.id, "1001");
        assert!((top.0 - 1.0).abs() < 1e-9);
        // Punctuation and case don't matter.
        assert!((list.fuzzy_by_name("SMITH, JOHN.", 10)[0].0 - 1.0).abs() < 1e-9);
        // Typos still rank the right client first, the near-miss second.
        assert_eq!(&ids("Jon Smith")[..2], ["1001", "1002"]);
        // Scores are sorted best first.
        let scores: Vec<f64> = list
            .fuzzy_by_name("Jones Mary", 10)
            .iter()
            .map(|s| s.0)
            .collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(ids("Jones Mary")[0], "1004");
        assert!(list.fuzzy_by_name("  ", 10).is_empty());
        // A limit returns the same best clients, in the same order.
        let all = ids("Jon Smith");
        let top2: Vec<String> = list
            .fuzzy_by_name("Jon Smith", 2)
            .iter()
            .map(|(_, c)| c.id.clone())
            .collect();
        assert_eq!(top2, all[..2]);
        assert!(list.fuzzy_by_name("Jon Smith", 0).is_empty());
    }

    #[test]
    fn five_thousand_rows_import_under_a_second() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::from("Client ID,Client Name,Entity Type,Partner,Email\n");
        for i in 0..5000 {
            text.push_str(&format!(
                "{},\"Client{i}, Person{i}\",1040,KW,c{i}@example.com\n",
                10_000 + i
            ));
        }
        let csv = write_csv(dir.path(), &text);
        let start = Instant::now();
        let list = apply_mapping(
            &read_csv(&csv).unwrap(),
            &map(
                "Client ID",
                "Client Name",
                &["Entity Type", "Partner", "Email"],
            ),
        )
        .unwrap();
        let took = start.elapsed();
        assert_eq!(list.len(), 5000);
        assert!(took < Duration::from_secs(1), "import took {took:?}");
        assert_eq!(
            list.match_by_id("14999").unwrap().name,
            "Client4999, Person4999"
        );
        assert_eq!(
            list.fuzzy_by_name("Person1234 Client1234", 10)[0].1.id,
            "11234"
        );
    }

    #[test]
    fn cache_round_trip_rebuilds_index() {
        let dir = tempfile::tempdir().unwrap();
        let list = list_of(&[("1001", "Smith, John")]);
        save_cache(dir.path(), &list).unwrap();
        let back = load_cache(dir.path()).unwrap().unwrap();
        assert_eq!(back.clients, list.clients);
        assert_eq!(back.match_by_id("1001").unwrap().name, "Smith, John");
        assert_eq!(back.fuzzy_by_name("John Smith", 10)[0].1.id, "1001");
        assert!(load_cache(&dir.path().join("none")).unwrap().is_none());
    }

    fn set_mtime(p: &Path, t: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn launch_reimports_only_when_csv_changed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let csv = write_csv(dir.path(), "id,name\n1001,A\n");
        let m = map("id", "name", &[]);
        let t0 = SystemTime::now() - Duration::from_secs(3600);
        set_mtime(&csv, t0);

        // No cache yet: imports.
        let first = load_on_launch(&data, Some(&csv), &m);
        let first_list = first.list.unwrap();
        assert_eq!(first_list.len(), 1);
        assert!(load_cache(&data).unwrap().is_some());

        // Same mtime: cached list used as-is.
        let second = load_on_launch(&data, Some(&csv), &m);
        assert!(second.note.is_none());
        assert_eq!(second.list.unwrap().imported_at, first_list.imported_at);

        // CSV edited: re-imported.
        std::fs::write(&csv, "id,name\n1001,A\n1002,B\n").unwrap();
        set_mtime(&csv, t0 + Duration::from_secs(60));
        let third = load_on_launch(&data, Some(&csv), &m);
        assert_eq!(third.list.unwrap().len(), 2);
        assert!(third.note.is_none(), "launch re-import must be silent");

        // CSV broken (duplicate IDs): keep the cached list, report the error.
        std::fs::write(&csv, "id,name\n1001,A\n1001,B\n").unwrap();
        set_mtime(&csv, t0 + Duration::from_secs(120));
        let fourth = load_on_launch(&data, Some(&csv), &m);
        assert_eq!(fourth.list.unwrap().len(), 2);
        let (level, msg) = fourth.note.unwrap();
        assert_eq!(level, Level::Error);
        assert!(msg.contains("duplicate client IDs: 1001"), "{msg}");
    }

    #[test]
    fn client_uuids_are_stable_across_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let csv = write_csv(dir.path(), "id,name\n1001,A\n1002,B\n");
        let m = map("id", "name", &[]);
        let t0 = SystemTime::now() - Duration::from_secs(3600);
        set_mtime(&csv, t0);
        let uuid_of = |l: &ClientList, id: &str| l.match_by_id(id).unwrap().uuid;

        let first = load_on_launch(&data, Some(&csv), &m).list.unwrap();
        let (a, b) = (uuid_of(&first, "1001"), uuid_of(&first, "1002"));
        assert!(!a.is_nil() && a != b);

        // 1002 leaves the CSV, then comes back: same uuid both times.
        std::fs::write(&csv, "id,name\n1001,A renamed\n").unwrap();
        set_mtime(&csv, t0 + Duration::from_secs(60));
        let second = load_on_launch(&data, Some(&csv), &m).list.unwrap();
        assert_eq!(uuid_of(&second, "1001"), a, "kept by CSV ID, not name");
        std::fs::write(&csv, "id,name\n1001,A\n1002,B\n1003,C\n").unwrap();
        set_mtime(&csv, t0 + Duration::from_secs(120));
        let third = load_on_launch(&data, Some(&csv), &m).list.unwrap();
        assert_eq!(uuid_of(&third, "1002"), b);
        assert!(!uuid_of(&third, "1003").is_nil());

        // A v0.1.0 cache (no uuids) gets them once, and they are saved (X115).
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cache_path(&data)).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("uuids");
        for c in json["clients"].as_array_mut().unwrap() {
            c.as_object_mut().unwrap().remove("uuid");
        }
        std::fs::write(cache_path(&data), json.to_string()).unwrap();
        let upgraded = load_cache(&data).unwrap().unwrap();
        let u = uuid_of(&upgraded, "1001");
        assert!(!u.is_nil());
        assert_eq!(uuid_of(&load_cache(&data).unwrap().unwrap(), "1001"), u);
    }

    #[test]
    fn launch_reimports_when_column_map_changed() {
        let dir = tempfile::tempdir().unwrap();
        let csv = write_csv(dir.path(), "id,name,Partner\n1001,A,KW\n");
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        load_on_launch(&data, Some(&csv), &map("id", "name", &[]));
        let with_partner = map("id", "name", &["Partner"]);
        let loaded = load_on_launch(&data, Some(&csv), &with_partner);
        let list = loaded.list.unwrap();
        assert_eq!(list.column_map, with_partner);
        assert_eq!(list.match_by_id("1001").unwrap().extra["Partner"], "KW");
    }

    /// Registers a job whose result the test sends by hand, so finishing order is controlled.
    fn fake_job(m: &mut ClientsManager, origin: Origin) -> std::sync::mpsc::Sender<Done> {
        let (tx, rx) = std::sync::mpsc::channel();
        let seq = m.next_seq;
        m.next_seq += 1;
        m.jobs.push(Job { seq, origin, rx });
        tx
    }

    fn imported(names: &[(&str, &str)]) -> Done {
        Done::Imported(Ok(list_of(names)))
    }

    #[test]
    fn older_job_finishing_later_does_not_replace_newer_list() {
        let mut m = ClientsManager::new();
        let launch = fake_job(&mut m, Origin::Launch);
        let wizard = fake_job(&mut m, Origin::Wizard);
        wizard.send(imported(&[("2", "New")])).unwrap();
        assert!(m.poll().is_some());
        assert!(m.busy());
        launch
            .send(Done::Loaded(Loaded {
                list: Some(list_of(&[("1", "Old")])),
                note: None,
            }))
            .unwrap();
        assert!(m.poll().is_none());
        assert!(!m.busy());
        assert_eq!(m.list().unwrap().clients[0].name, "New");
        assert_eq!(m.take_wizard_result(), Some(Ok(())));
    }

    #[test]
    fn results_arriving_together_apply_in_start_order() {
        let mut m = ClientsManager::new();
        let first = fake_job(&mut m, Origin::Refresh);
        let second = fake_job(&mut m, Origin::Refresh);
        second.send(imported(&[("2", "Second")])).unwrap();
        first.send(imported(&[("1", "First")])).unwrap();
        m.poll();
        assert_eq!(m.list().unwrap().clients[0].name, "Second");
    }

    #[test]
    fn wizard_failure_goes_to_wizard_not_setup_note() {
        let mut m = ClientsManager::new();
        let tx = fake_job(&mut m, Origin::Wizard);
        tx.send(Done::Imported(Err("duplicate client IDs: 1".into())))
            .unwrap();
        m.poll();
        assert!(m.note().is_none());
        assert_eq!(
            m.take_wizard_result(),
            Some(Err("duplicate client IDs: 1".into()))
        );
        // A job whose thread died is reported, not left busy forever.
        let refresh = fake_job(&mut m, Origin::Refresh);
        drop(refresh);
        m.poll();
        assert!(!m.busy());
        assert_eq!(m.note().unwrap().0, Level::Error);
    }

    #[test]
    fn launch_without_csv_uses_cache() {
        let dir = tempfile::tempdir().unwrap();
        save_cache(dir.path(), &list_of(&[("1", "A")])).unwrap();
        let loaded = load_on_launch(dir.path(), None, &ColumnMap::default());
        assert_eq!(loaded.list.unwrap().len(), 1);
        assert!(loaded.note.is_none());
    }
}
