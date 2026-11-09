//! Top-level egui app: shared state, tab routing, the close prompt, window-geometry persistence.

use std::path::PathBuf;

use crate::clients::ClientsManager;
use crate::commit::{self, LastBatch};
use crate::config::{Settings, WindowState};
use crate::intake::IntakeManager;
use crate::pdf::Sniffer;
use crate::recovery::{self, Recovery};
use crate::rules::{self, RulesFile};
use crate::sentinel::{FoldersManager, ScanInput, Status};
use crate::template::{self, TemplateNode};
use crate::ui::chrome;
use crate::ui::conflicts::ConflictsScreen;
use crate::ui::history::HistoryScreen;
use crate::ui::import::ImportScreen;
use crate::ui::preview::PreviewScreen;
use crate::ui::relationships::RelationshipsScreen;
use crate::ui::rules::RulesScreen;
use crate::ui::setup::SetupScreen;
use crate::ui::widgets::show_in_explorer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Preview,
    Setup,
    Rules,
    Import,
    History,
    Conflicts,
    Relationships,
}

impl Tab {
    /// Header order (UI_SPEC §2).
    pub const ALL: [Tab; 7] = [
        Tab::Preview,
        Tab::Setup,
        Tab::Rules,
        Tab::Import,
        Tab::History,
        Tab::Conflicts,
        Tab::Relationships,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Preview => "Preview",
            Tab::Setup => "Setup",
            Tab::Rules => "Rules",
            Tab::Import => "Import",
            Tab::History => "History",
            Tab::Conflicts => "Conflicts",
            Tab::Relationships => "Relationships",
        }
    }
}

/// A zip sent to the queue (P07b): its extracted files, waiting to be filed.
#[derive(Debug, Clone)]
pub struct RoutedZip {
    pub id: uuid::Uuid,
    pub zip: PathBuf,
    /// The zip as it was listed on Import, to list it again if nothing gets filed.
    pub listed: Option<crate::intake::QueuedFile>,
    /// data/tmp/zip-<id>
    pub tmp: PathBuf,
    pub entries: Vec<PathBuf>,
    /// The queued files, to queue them again when an undo brings one back.
    pub files: Vec<crate::intake::QueuedFile>,
    /// Entries filed so far and the batch that filed each (directly, as split pieces or
    /// combined); undoing that batch takes them out again.
    pub filed: Vec<(uuid::Uuid, PathBuf)>,
}

/// Saved settings and data every screen works with.
pub struct AppState {
    pub data_dir: PathBuf,
    pub settings: Settings,
    /// False when config.json existed but could not be read; it is then never overwritten
    /// except by an explicit Save (which sets the old file aside).
    pub settings_savable: bool,
    pub template: TemplateNode,
    /// Same as `settings_savable`, for template.json.
    pub template_savable: bool,
    pub template_error: Option<String>,
    pub rules: RulesFile,
    /// Same as `settings_savable`, for rules.json.
    pub rules_savable: bool,
    pub rules_error: Option<String>,
    pub clients: ClientsManager,
    pub folders: FoldersManager,
    /// Inbox watcher and file queue. Started by [`FilerApp::new`].
    pub intake: IntakeManager,
    /// First-page text reader for doc-type sniffing. Started by [`FilerApp::new`].
    pub sniffer: Sniffer,
    /// The most recent commit (for undo).
    pub last_batch: Option<LastBatch>,
    /// Leftovers from the previous session (shown until dismissed).
    pub recovery: Recovery,
    /// Bumped whenever ops.jsonl gains entries, so History reloads.
    pub ops_generation: u64,
    /// A commit is filing (set by the app each frame); Undo waits for it.
    pub committing: bool,
    /// A screen asks to open another tab (e.g. the "Conflicts" link in Setup).
    pub goto: Option<Tab>,
    /// A file name for the Rules tab's test box ("Build a rule from this file…", V10).
    pub rule_test: Option<String>,
    /// Parent → child relationships (P07a), saved on every edit.
    pub relationships: crate::relationships::Relationships,
    /// False when relationships.json could not be read (never written over then).
    pub relationships_savable: bool,
    pub relationships_error: Option<String>,
    pub relationships_saved_at: Option<chrono::DateTime<chrono::Local>>,
    /// Bumped on every relationship edit, so plans that use them are rebuilt.
    pub relationships_generation: u64,
    /// "Add relationship…" from Preview: (parent ID, child name or ID to look for). The
    /// Relationships tab opens on the parent with the add row filled, and returns to Preview.
    pub relationship_prefill: Option<(String, String)>,
    /// "Show in History" from Import: filter History to this batch.
    pub history_filter: Option<uuid::Uuid>,
    /// "Show import" from History: open this import's detail on the Import tab.
    pub import_show: Option<uuid::Uuid>,
    /// Zips sent to the queue whose entries are still being filed (P07b).
    pub routed: Vec<RoutedZip>,
    /// The last batch (and whether it was undone) already counted against `routed`.
    routed_seen: Option<(uuid::Uuid, bool)>,
    /// Folder of a pdfium.dll that bound at startup; `None` disables PDF features.
    pub pdfium_dir: Option<PathBuf>,
    /// Set by a host that keeps its own settings lock (Vibe Files' admin password): Setup is
    /// shown read-only with this reason. `None`: Setup works as always.
    pub setup_locked: Option<String>,
    /// A host that can create clients (Vibe Files): rows whose client is not found offer
    /// "New client…", which hands the host a [`NewClientRequest`]. Off on its own.
    pub host_new_client: bool,
    /// The row a "New client…" was pressed on, for the host to take.
    pub new_client_request: Option<NewClientRequest>,
    /// A host with its own trash (Vibe Files): rows offer "Move to trash", which hands the host
    /// the files ([`FilerApp::take_trash_request`]). Off on its own: Filer never deletes a file.
    pub host_trash: bool,
    /// Files whose "Move to trash" was pressed, for the host to take.
    pub trash_request: Option<Vec<PathBuf>>,
    /// The host shows files itself (Vibe Files' Viewer): a double-clicked file name is handed
    /// to it ([`FilerApp::take_open_request`]) instead of opening in the Windows program.
    pub host_open: bool,
    /// The file whose name was double-clicked, for the host to show.
    pub open_request: Option<PathBuf>,
    /// What the host read from a file (Vibe Files' form reading), shown on its row.
    pub host_notes: std::collections::HashMap<PathBuf, String>,
    /// This person's own inboxes (owner 2026-11-05: several people on one Remote Desktop
    /// server), from a host that keeps them per Windows login. Watched besides the shared ones,
    /// never saved into the shared settings; Preview shows them on a "My inbox" tab.
    pub personal_inboxes: Vec<PathBuf>,
}

/// "New client…" pressed on a row (host only, [`AppState::host_new_client`]): the file, and
/// what its name says about the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClientRequest {
    pub file: PathBuf,
    /// The client name part of the file name (or what was typed in the picker).
    pub name: String,
    /// The client ID in the file name, when there is one.
    pub id: Option<String>,
}

impl AppState {
    /// The shared inboxes, then this person's own (each once).
    pub fn watched_inboxes(&self) -> Vec<PathBuf> {
        let mut v = self.settings.inboxes.clone();
        for p in &self.personal_inboxes {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
        v
    }

    /// Whether a queued file came from one of this person's own inboxes.
    pub fn is_personal(&self, q: &crate::intake::QueuedFile) -> bool {
        matches!(&q.source, crate::intake::Source::Inbox(i) if self.personal_inboxes.contains(i))
            && !self
                .settings
                .inboxes
                .iter()
                .any(|s| matches!(&q.source, crate::intake::Source::Inbox(i) if i == s))
    }

    /// Loads the template and folder index from `data_dir` (small local files).
    pub fn load(
        data_dir: PathBuf,
        settings: Settings,
        settings_savable: bool,
        pdfium_dir: Option<PathBuf>,
    ) -> Self {
        let mut settings = settings;
        // The firm ID goes into every sentinel, so it is saved before anything can use it.
        if settings.ensure_firm_id() {
            if !settings_savable {
                tracing::warn!("firm_id generated but config.json can't be saved over");
            } else if let Err(e) = settings.save(&data_dir) {
                tracing::error!(error = %e, "could not save the new firm_id to config.json");
            }
        }
        let (template, template_savable, template_error) = template::load_or_seed(&data_dir);
        let folders = FoldersManager::load(&data_dir);
        let (rules, rules_savable, rules_error) = rules::load_or_seed(&data_dir);
        let last_batch = commit::load_last_batch(&data_dir);
        let recovery = Recovery::scan(&data_dir, last_batch.as_ref());
        let (relationships, relationships_savable, relationships_error) =
            crate::relationships::load(&data_dir);
        let relationships_saved_at = std::fs::metadata(crate::relationships::path(&data_dir))
            .and_then(|m| m.modified())
            .ok()
            .map(chrono::DateTime::from);
        Self {
            data_dir,
            settings,
            settings_savable,
            template,
            template_savable,
            template_error,
            rules,
            rules_savable,
            rules_error,
            clients: ClientsManager::new(),
            folders,
            intake: IntakeManager::new(),
            sniffer: Sniffer::disabled(),
            last_batch,
            recovery,
            ops_generation: 0,
            committing: false,
            goto: None,
            rule_test: None,
            relationships,
            relationships_savable,
            relationships_error,
            relationships_saved_at,
            relationships_generation: 0,
            relationship_prefill: None,
            history_filter: None,
            import_show: None,
            routed: Vec::new(),
            routed_seen: None,
            pdfium_dir,
            setup_locked: None,
            host_new_client: false,
            new_client_request: None,
            host_trash: false,
            trash_request: None,
            host_open: false,
            open_request: None,
            host_notes: Default::default(),
            personal_inboxes: Vec::new(),
        }
    }

    /// Counts a newly committed (or undone) last batch against the zips sent to the queue: which
    /// of their entries it filed, directly, as split pieces or combined. An undo takes those
    /// entries out again and queues the ones it brought back.
    fn count_routed_batch(&mut self) {
        let Some(lb) = self.last_batch.as_ref().filter(|lb| !lb.in_progress) else {
            return;
        };
        let key = (lb.batch, lb.undone);
        if self.routed_seen == Some(key) {
            return;
        }
        self.routed_seen = Some(key);
        let queued: Vec<PathBuf> = self.intake.queue().iter().map(|q| q.path.clone()).collect();
        let mut back = Vec::new();
        for r in &mut self.routed {
            if lb.undone {
                let undone: Vec<PathBuf> = r
                    .filed
                    .iter()
                    .filter(|(b, _)| *b == lb.batch)
                    .map(|(_, e)| e.clone())
                    .collect();
                r.filed.retain(|(b, _)| *b != lb.batch);
                for e in undone {
                    if e.is_file()
                        && !queued.contains(&e)
                        && let Some(f) = r.files.iter().find(|f| f.path == e)
                    {
                        back.push(f.clone());
                    }
                }
            } else {
                for i in &lb.items {
                    let sources = std::iter::once(&i.src)
                        .chain(i.piece_of.iter())
                        .chain(i.merged.iter().map(|m| &m.original));
                    for s in sources {
                        if r.entries.contains(s) && !r.filed.iter().any(|(_, e)| e == s) {
                            r.filed.push((lb.batch, s.clone()));
                        }
                    }
                }
            }
        }
        if !back.is_empty() {
            self.intake.add_routed(back);
        }
    }

    /// A zip sent to the queue is done once none of its entries is queued any more (P07b). If the
    /// last batch filed some of them, a verified copy of the zip is kept for that batch's undo and
    /// the zip is removed. If they were all filed in earlier batches the zip stays where it is (its
    /// undo window has passed; nothing is deleted without one). If none was filed it goes back on
    /// the Import list. The temp folder goes if empty. Call every frame (cheap when idle).
    pub fn finish_routed_zips(&mut self) {
        if self.routed.is_empty() {
            return;
        }
        self.count_routed_batch();
        let queue: Vec<PathBuf> = self.intake.queue().iter().map(|q| q.path.clone()).collect();
        let (done, waiting): (Vec<RoutedZip>, Vec<RoutedZip>) = std::mem::take(&mut self.routed)
            .into_iter()
            .partition(|r| !r.entries.iter().any(|e| queue.contains(e)));
        self.routed = waiting;
        for r in done {
            let filed = r.filed.len();
            let current = self
                .last_batch
                .as_ref()
                .filter(|lb| !lb.undone && !lb.in_progress)
                .map(|lb| lb.batch)
                .filter(|b| r.filed.iter().any(|(fb, _)| fb == b));
            let mut note = format!("queue · {} queued · {filed} filed", r.entries.len());
            let mut result = crate::oplog::OpResult::Ok;
            let mut error = None;
            match (current, self.last_batch.as_mut()) {
                (Some(_), Some(lb)) => {
                    let dir = self.data_dir.join(commit::UNDO_ZIPS_DIR);
                    let kept = dir.join(format!("{}-{}.zip", lb.batch, lb.routed_zips.len() + 1));
                    // A rename on one volume; elsewhere a verified copy, then the zip goes.
                    let keep = std::fs::create_dir_all(&dir)
                        .map_err(|e| e.to_string())
                        .and_then(|()| commit::transfer(&r.zip, &kept, false));
                    match keep {
                        Ok(_) => {
                            lb.routed_zips.push(commit::ZipSource {
                                original: r.zip.clone(),
                                kept,
                            });
                            note.push_str(" · zip deleted, undo copy kept");
                            if let Err(e) = commit::save_last_batch(&self.data_dir, lb) {
                                tracing::error!(error = %e, "last_batch.json could not be written");
                            }
                        }
                        Err(e) => {
                            note.push_str(" · zip kept");
                            result = crate::oplog::OpResult::Error;
                            error = Some(format!("the zip could not be removed: {e}"));
                        }
                    }
                }
                _ if filed > 0 => {
                    note.push_str(" · zip kept (its files were filed in an earlier batch)");
                }
                _ => {
                    note.push_str(" · zip kept");
                    if r.zip.is_file()
                        && let Some(f) = r.listed.clone()
                    {
                        self.intake.relist_zip(f);
                    }
                }
            }
            let _ = std::fs::remove_dir(&r.tmp);
            let rec = crate::oplog::OpRecord {
                src: Some(r.zip.clone()),
                dst: Some(r.tmp.clone()),
                result,
                error,
                note: Some(note),
                ..crate::oplog::OpRecord::new(current.unwrap_or(r.id), crate::oplog::Op::ZipImport)
            };
            if let Err(e) = crate::oplog::append(&crate::oplog::path(&self.data_dir), &rec) {
                tracing::error!(error = %e, "cannot append to ops.jsonl");
            }
            self.ops_generation += 1;
        }
    }

    /// Writes relationships.json after an edit (every edit is saved). Refuses while the file on
    /// disk could not be read, so it is never written over.
    pub fn save_relationships(&mut self) -> Result<(), String> {
        if !self.relationships_savable {
            return Err(
                "relationships.json could not be read at startup; fix or move it, then restart."
                    .into(),
            );
        }
        crate::relationships::save(&self.data_dir, &self.relationships)
            .map_err(|e| format!("Could not save relationships.json: {e}"))?;
        self.relationships_saved_at = Some(chrono::Local::now());
        self.relationships_generation += 1;
        Ok(())
    }

    pub fn pdf_enabled(&self) -> bool {
        self.pdfium_dir.is_some()
    }

    /// Folders in the last scan of the saved root that are neither bound nor ignored.
    pub fn conflict_count(&self) -> usize {
        self.folders
            .scan_of(self.settings.root.as_deref())
            .map_or(0, |f| {
                f.folders
                    .iter()
                    .filter(|b| !matches!(b.status, Status::Bound | Status::Ignored))
                    .count()
            })
    }

    /// Why the root can't be scanned right now, if it can't.
    pub fn scan_blocker(&self) -> Option<&'static str> {
        if self.settings.root.is_none() {
            Some("Choose and save the client root in Setup first.")
        } else if self.clients.list().is_none() {
            Some("Import the client list in Setup first.")
        } else if self.folders.scanning().is_some() {
            Some("A scan is already running.")
        } else {
            None
        }
    }

    /// Rescans the saved root with the current client list and matching settings.
    pub fn start_scan(&mut self, ctx: &egui::Context) {
        if self.scan_blocker().is_some() {
            return;
        }
        let (Some(root), Some(clients)) =
            (self.settings.root.clone(), self.clients.list().cloned())
        else {
            return;
        };
        self.folders.start_scan(
            ctx,
            self.data_dir.clone(),
            ScanInput {
                root,
                clients,
                id_pattern: self.settings.id_pattern.clone(),
                threshold: self.settings.fuzzy_suggest,
                ignored: Vec::new(),
            },
        );
    }
}

enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

pub struct FilerApp {
    shared: AppState,
    tab: Tab,
    setup: SetupScreen,
    rules: RulesScreen,
    preview: PreviewScreen,
    history: HistoryScreen,
    conflicts: ConflictsScreen,
    relationships: RelationshipsScreen,
    import: ImportScreen,
    /// The window was asked to close while Setup or Rules had unsaved edits.
    close_prompt: bool,
    /// The user chose Save or Discard in the close prompt; let the next close through.
    allow_close: bool,
    /// "Delete leftovers" confirmation is open.
    recovery_confirm: bool,
    recovery_message: Option<String>,
    /// Theme and zoom last applied to the context; a change on either side is saved (V1, V2).
    applied: crate::config::Display,
}

impl FilerApp {
    /// Locks Setup (shown read-only with `reason`) or unlocks it (`None`): for a host that keeps
    /// its own settings lock.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_setup_lock(&mut self, reason: Option<String>) {
        self.shared.setup_locked = reason;
    }

    /// Lets rows whose client is not found offer "New client…" (see [`Self::take_new_client_request`]).
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn offer_new_client(&mut self, on: bool) {
        self.shared.host_new_client = on;
    }

    /// Lets rows (and ticked rows) offer "Move to trash" (see [`Self::take_trash_request`]).
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn offer_trash(&mut self, on: bool) {
        self.shared.host_trash = on;
    }

    /// The host shows double-clicked files itself (V24): Preview hands them over through
    /// [`Self::take_open_request`] instead of opening them in the Windows program.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn offer_open(&mut self, on: bool) {
        self.shared.host_open = on;
    }

    /// The file whose name was double-clicked in Preview, once (host only).
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn take_open_request(&mut self) -> Option<PathBuf> {
        self.shared.open_request.take()
    }

    /// The files whose "Move to trash" was pressed, once. The host moves them out of the inbox,
    /// then calls [`Self::files_taken`].
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn take_trash_request(&mut self) -> Option<Vec<PathBuf>> {
        self.shared.trash_request.take()
    }

    /// The host took `files` out of the inbox: they leave the queue and their rows forget them.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn files_taken(&mut self, files: &[PathBuf]) {
        self.preview.forget(files);
        for f in files {
            self.shared.host_notes.remove(f);
            self.shared.intake.remove(f);
        }
    }

    /// This person's own inboxes (a host that keeps them per Windows login): watched besides the
    /// shared ones, shown on Preview's "My inbox" tab, never saved into the shared settings.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_personal_inboxes(&mut self, inboxes: Vec<PathBuf>) {
        if self.shared.personal_inboxes != inboxes {
            self.shared.personal_inboxes = inboxes;
            if self.shared.personal_inboxes.is_empty() {
                self.preview.show_mine(false);
            }
        }
    }

    /// Rows whose file name no rule names, for a host to read (Vibe Files' form reading).
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn unnamed_files(&self) -> Vec<PathBuf> {
        self.preview.unnamed_files()
    }

    /// What the host read from `file`: the client and year when its row has none (nothing a
    /// person chose is changed), and a note shown on the row.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn suggest(
        &mut self,
        file: &std::path::Path,
        client_id: Option<&str>,
        year: Option<u16>,
        note: &str,
    ) {
        self.preview.suggest(file, client_id, year);
        if !note.is_empty() {
            self.shared
                .host_notes
                .insert(file.to_path_buf(), note.to_string());
        }
    }

    /// The row whose "New client…" was pressed, once.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn take_new_client_request(&mut self) -> Option<NewClientRequest> {
        self.shared.new_client_request.take()
    }

    /// The host created client `client_id` (in `clients.json`) for `file`: the client list is
    /// read again and the row gets that client.
    // Used by a host that embeds Filer (Vibe Files), not by this program.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn client_created(&mut self, ctx: &egui::Context, file: &std::path::Path, client_id: &str) {
        let s = &self.shared;
        self.shared.clients.load(
            ctx,
            s.data_dir.clone(),
            s.settings.csv_path.clone(),
            s.settings.column_map.clone(),
        );
        self.preview.set_client(file, client_id);
    }

    pub fn new(ctx: &egui::Context, mut shared: AppState) -> Self {
        crate::ui::theme::install(ctx);
        let setup = SetupScreen::new(&shared);
        let rules = RulesScreen::new(&shared);
        shared.intake.start(ctx);
        shared.sniffer = Sniffer::new(ctx, shared.pdfium_dir.clone());
        let watched = shared.watched_inboxes();
        shared.intake.set_file_types(&shared.settings.file_types);
        shared
            .intake
            .set_inboxes(&watched, shared.settings.inbox_subfolders);
        shared.clients.load(
            ctx,
            shared.data_dir.clone(),
            shared.settings.csv_path.clone(),
            shared.settings.column_map.clone(),
        );
        // First run goes to Setup; once a root is set the app opens on Preview.
        let tab = if shared.settings.root.is_some() {
            Tab::Preview
        } else {
            Tab::Setup
        };
        Self {
            shared,
            tab,
            setup,
            rules,
            preview: PreviewScreen::default(),
            history: HistoryScreen::default(),
            conflicts: ConflictsScreen::default(),
            relationships: RelationshipsScreen::default(),
            import: ImportScreen::default(),
            close_prompt: false,
            allow_close: false,
            recovery_confirm: false,
            recovery_message: None,
            applied: crate::config::Display {
                zoom: 0.0,
                ..Default::default()
            },
        }
    }

    /// Applies the saved theme and zoom, and saves them when they change: the header's theme
    /// button and Setup's font size edit the settings, Ctrl+Plus / Ctrl+Minus / Ctrl+0 (egui's
    /// built-in zoom keys) edit the context.
    fn sync_display(&mut self, ctx: &egui::Context) {
        use crate::config::{Display, ThemeChoice};
        let first = self.applied.zoom == 0.0;
        let mut want = self.shared.settings.display;
        let ctx_zoom = ctx.zoom_factor();
        if !first && (ctx_zoom - self.applied.zoom).abs() > 0.001 {
            want.zoom = ctx_zoom;
        }
        want.zoom = Display::clamp_zoom(want.zoom);
        if (ctx_zoom - want.zoom).abs() > 0.001 {
            ctx.set_zoom_factor(want.zoom);
        }
        if first || want.theme != self.applied.theme {
            ctx.set_theme(match want.theme {
                ThemeChoice::System => egui::ThemePreference::System,
                ThemeChoice::Light => egui::ThemePreference::Light,
                ThemeChoice::Dark => egui::ThemePreference::Dark,
            });
        }
        let changed = !first && want != self.applied;
        self.applied = want;
        self.shared.settings.display = want;
        if changed
            && self.shared.settings_savable
            && let Err(e) = self.shared.settings.save(&self.shared.data_dir)
        {
            tracing::error!(error = %e, "saving config.json failed");
        }
    }

    fn unsaved_tabs(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.setup.is_dirty(&self.shared) {
            out.push("Setup");
        }
        if self.rules.is_dirty(&self.shared) {
            out.push("Rules");
        }
        out
    }

    /// Save / Discard / Cancel for unsaved Setup or Rules edits when the window is closed.
    fn close_dialog(&mut self, ctx: &egui::Context) {
        let tabs = self.unsaved_tabs().join(" and ");
        let can_save = (!self.setup.is_dirty(&self.shared) || self.setup.can_save())
            && (!self.rules.is_dirty(&self.shared) || self.rules.can_save(&self.shared));
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("unsaved_setup")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            ui.label(format!("Save your {tabs} changes before closing?"));
            if !can_save {
                ui.label(format!(
                    "They can't be saved until the errors on the {tabs} tab are fixed."
                ));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_save, egui::Button::new("Save"))
                    .clicked()
                {
                    choice = Some(CloseChoice::Save);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(CloseChoice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(CloseChoice::Cancel);
                }
            });
        });
        if choice.is_none() && modal.should_close() {
            choice = Some(CloseChoice::Cancel);
        }
        match choice {
            Some(CloseChoice::Save) => {
                self.close_prompt = false;
                let setup_ok =
                    !self.setup.is_dirty(&self.shared) || self.setup.save(&mut self.shared);
                if setup_ok && self.rules.save(&mut self.shared) {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Some(CloseChoice::Discard) => {
                self.close_prompt = false;
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(CloseChoice::Cancel) => self.close_prompt = false,
            None => {}
        }
    }

    /// An import was saved to `clients.json`: store its CSV path and column map in config.json.
    /// If that fails, they stay as unsaved Setup edits with an error, so nothing is lost silently.
    fn record_import(&mut self, done: crate::clients::Committed) {
        let s = &mut self.shared;
        let mut next = s.settings.clone();
        next.csv_path = Some(done.source.clone());
        next.column_map = done.column_map.clone();
        let saved = if s.settings_savable {
            next.save(&s.data_dir).map_err(|e| e.to_string())
        } else {
            Err("config.json could not be read at startup".to_string())
        };
        match saved {
            Ok(()) => {
                s.settings = next;
                self.setup.sync_csv(&s.settings);
            }
            Err(e) => {
                tracing::error!(error = %e, "storing the client CSV in config.json failed");
                self.setup.stage_csv(done.source, done.column_map, &e);
            }
        }
    }

    /// Files or folders dropped anywhere on the window go to the queue; the Preview tab opens.
    /// While files are dragged over the window, a hint is drawn on top.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let (hovering, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect::<Vec<_>>(),
            )
        });
        if !dropped.is_empty() {
            tracing::info!(count = dropped.len(), "files dropped on the window");
            // Zips go to Import (P07b); anything else to Preview.
            let all_zips = dropped
                .iter()
                .all(|p| crate::intake::kind_of(p) == crate::intake::Kind::Zip);
            self.shared.intake.add_paths(dropped);
            self.tab = if all_zips { Tab::Import } else { Tab::Preview };
        }
        if hovering {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop_hint"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to add to the queue",
                egui::FontId::proportional(24.0),
                egui::Color32::WHITE,
            );
        }
    }

    /// What the previous session left behind, above the tabs (X103).
    fn recovery_ui(&mut self, ui: &mut egui::Ui) {
        let r = &self.shared.recovery;
        if r.is_empty() && self.recovery_message.is_none() {
            return;
        }
        let data_dir = self.shared.data_dir.clone();
        let mut dismiss = false;
        egui::Panel::top("recovery").show(ui, |ui| {
            ui.add_space(4.0);
            if !r.leftovers.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "The last session left {} temporary PDF file{} in data\\tmp (combinations or splits that were never filed).",
                        r.leftovers.len(),
                        if r.leftovers.len() == 1 { "" } else { "s" }
                    ));
                    if ui.small_button("Open folder").clicked() {
                        show_in_explorer(&data_dir.join(crate::pdf::TMP_DIR));
                    }
                    if ui.small_button("Delete leftovers…").clicked() {
                        self.recovery_confirm = true;
                    }
                });
            }
            if let Some(b) = &r.unfinished {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!(
                            "The commit of {} did not finish: {} file{} had been filed. Check History; Undo is not available for it.",
                            b.committed_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
                            b.items.len(),
                            if b.items.len() == 1 { "" } else { "s" }
                        ),
                    );
                    if ui.small_button("Open data folder").clicked() {
                        show_in_explorer(&data_dir);
                    }
                });
            }
            if let Some(m) = &self.recovery_message {
                ui.label(m);
            }
            if ui.small_button("Dismiss").clicked() {
                dismiss = true;
            }
            ui.add_space(4.0);
        });
        if dismiss {
            self.shared.recovery = Recovery::default();
            self.recovery_message = None;
        }
    }

    fn recovery_confirm_ui(&mut self, ctx: &egui::Context) {
        if !self.recovery_confirm {
            return;
        }
        let n = self.shared.recovery.leftovers.len();
        let mut delete = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("recovery_confirm")).show(ctx, |ui| {
            ui.heading("Delete leftovers");
            ui.label(format!(
                "Delete {n} temporary PDF file{} in data\\tmp? They are copies the app made; originals in your inboxes are not touched.",
                if n == 1 { "" } else { "s" }
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete files").clicked() {
                    delete = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if delete {
            // Exactly the files the panel listed; anything a running combine or split is
            // writing now (and the session's own temp files) stays.
            let keep = self.preview.temp_files();
            let listed: Vec<PathBuf> = self
                .shared
                .recovery
                .leftovers
                .iter()
                .filter(|p| !keep.contains(p))
                .cloned()
                .collect();
            let (deleted, problems) = recovery::delete_leftovers(&self.shared.data_dir, &listed);
            self.shared.recovery.leftovers.retain(|p| p.exists());
            self.recovery_message = Some(if problems.is_empty() {
                format!(
                    "Deleted {deleted} temporary file{}.",
                    if deleted == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "Deleted {deleted}; could not delete: {}",
                    problems.join(" | ")
                )
            });
            self.recovery_confirm = false;
        } else if close || modal.should_close() {
            self.recovery_confirm = false;
        }
    }

    /// Enter (Preview: open the Commit summary), Del (Preview: skip the row under the mouse),
    /// Ctrl+Z (Undo prompt). Ignored while typing or while a dialog is open.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || ctx.memory(|m| m.top_modal_layer()).is_some() {
            return;
        }
        let (enter, delete, undo) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
            )
        });
        if undo {
            self.tab = Tab::History;
            self.history.request_undo(&self.shared);
        }
        if self.tab == Tab::Preview {
            if enter {
                self.preview.press_enter();
            }
            if delete {
                self.preview.press_delete();
            }
        }
    }

    /// Header Profile dropdown (X120).
    fn profile_choice(&self) -> chrome::ProfileChoice {
        chrome::ProfileChoice {
            names: self.shared.rules.profiles.keys().cloned().collect(),
            active: self.shared.rules.active.clone(),
            locked: self
                .rules
                .is_dirty(&self.shared)
                .then_some("Save or revert the Rules tab first."),
        }
    }

    /// Right-hand footer text for the open screen (UI_SPEC §2).
    fn footer_context(&self) -> String {
        let s = &self.shared;
        match self.tab {
            Tab::Preview if let Some(n) = self.preview.split_thumbs_in_memory() => {
                format!("thumbnail cache {n} / {}", crate::ui::split::THUMB_CACHE)
            }
            Tab::Preview => match &s.last_batch {
                Some(b) if !b.undone && !b.in_progress => format!(
                    "Undo available: batch {}",
                    b.committed_at
                        .with_timezone(&chrono::Local)
                        .format("%-I:%M %p")
                ),
                _ => String::new(),
            },
            Tab::Setup => format!("data: {}", s.data_dir.display()),
            Tab::Rules => self
                .rules
                .saved_at()
                .map(|t| format!("rules.json saved {}", t.format("%H:%M")))
                .unwrap_or_default(),
            Tab::History => format!("ops.jsonl · {} lines", self.history.record_count()),
            Tab::Conflicts => s
                .folders
                .scan_of(s.settings.root.as_deref())
                .map(|f| format!("folders.json · {} entries", f.folders.len()))
                .unwrap_or_default(),
            Tab::Relationships => s
                .relationships_saved_at
                .map(|t| format!("relationships.json saved {}", t.format("%-I:%M %p")))
                .unwrap_or_default(),
            Tab::Import => self.import.footer(),
        }
    }

    fn track_window(&mut self, ctx: &egui::Context) {
        let state = ctx.input(|i| {
            let vp = i.viewport();
            if vp.minimized == Some(true) || vp.maximized == Some(true) {
                return None;
            }
            let outer = vp.outer_rect?;
            let inner = vp.inner_rect?;
            Some(WindowState {
                x: outer.min.x,
                y: outer.min.y,
                width: inner.width(),
                height: inner.height(),
            })
        });
        if state.is_some() {
            self.shared.settings.window = state;
        }
    }
}

impl eframe::App for FilerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.sync_display(&ctx);
        self.track_window(&ctx);
        if let Some(done) = self.shared.clients.poll() {
            self.record_import(done);
        }
        let data_dir = self.shared.data_dir.clone();
        self.shared.folders.poll(&ctx, &data_dir);
        let inboxes = self.shared.watched_inboxes();
        let subfolders = self.shared.settings.inbox_subfolders;
        self.shared.intake.set_inboxes(&inboxes, subfolders);
        let types = self.shared.settings.file_types.clone();
        self.shared.intake.set_file_types(&types);
        // A note on a file that left the queue (filed, skipped, taken) goes with it.
        if !self.shared.host_notes.is_empty() {
            let queue = self.shared.intake.queue();
            self.shared
                .host_notes
                .retain(|p, _| queue.iter().any(|q| &q.path == p));
        }
        self.shared.intake.poll();
        self.shared.sniffer.poll();
        self.handle_dropped_files(&ctx);
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && !self.unsaved_tabs().is_empty()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_prompt = true;
            self.tab = if self.setup.is_dirty(&self.shared) {
                Tab::Setup
            } else {
                Tab::Rules
            };
        }

        let badges = chrome::Badges {
            preview: self.shared.intake.queue().len(),
            import: ImportScreen::draft_count(&self.shared),
            conflicts: self.shared.conflict_count(),
        };
        self.shortcuts(&ctx);
        let profiles = self.profile_choice();
        let picked = chrome::header(
            ui,
            &mut self.tab,
            badges,
            profiles,
            &mut self.shared.settings.display.theme,
        );
        if let Some(name) = picked {
            self.rules.switch_profile(&mut self.shared, name);
        }
        let context = self.footer_context();
        chrome::footer(
            ui,
            chrome::Footer {
                root: self.shared.settings.root.as_deref(),
                inboxes: self.shared.settings.inboxes.len(),
                clients: self.shared.clients.list().map(|l| l.len()),
                relationships: self.shared.relationships.len(),
                pdfium: self.shared.pdfium_dir.is_some(),
                context,
            },
        );
        self.recovery_ui(ui);

        self.shared.committing = self.preview.committing();
        if !self.shared.committing {
            self.shared.finish_routed_zips();
        }
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Setup => match self.shared.setup_locked.clone() {
                // Locked by the host: shown, nothing can be changed.
                Some(why) => {
                    crate::ui::theme::banner(ui, crate::config::Level::Info, &why);
                    ui.add_enabled_ui(false, |ui| self.setup.show(ui, &mut self.shared));
                }
                None => self.setup.show(ui, &mut self.shared),
            },
            Tab::Rules => self.rules.show(ui, &mut self.shared),
            Tab::Preview => self.preview.show(ui, &mut self.shared),
            Tab::History => self.history.show(ui, &mut self.shared),
            Tab::Conflicts => self.conflicts.show(ui, &mut self.shared),
            Tab::Import => self.import.show(ui, &mut self.shared),
            Tab::Relationships => self.relationships.show(ui, &mut self.shared),
        });

        if let Some(t) = self.shared.goto.take() {
            self.tab = t;
        }
        if self.close_prompt {
            self.close_dialog(&ctx);
        }
        self.recovery_confirm_ui(&ctx);
    }

    fn on_exit(&mut self) {
        if !self.shared.settings_savable {
            return;
        }
        if let Err(e) = self.shared.settings.save(&self.shared.data_dir) {
            tracing::error!(error = %e, "saving config.json failed");
        }
    }
}

/// Saves a rendered image of the harness to `$VFD_SNAPSHOT_DIR/<name>.png` when that variable is
/// set (for looking at screens automation can't click to). Does nothing otherwise.
#[cfg(test)]
pub fn snapshot<S>(h: &mut egui_kittest::Harness<'_, S>, name: &str) {
    let Ok(dir) = std::env::var("VFD_SNAPSHOT_DIR") else {
        return;
    };
    match h.render() {
        Ok(img) => img
            .save(std::path::Path::new(&dir).join(format!("{name}.png")))
            .expect("write snapshot"),
        Err(e) => eprintln!("snapshot {name} failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{ViewportCommand, ViewportEvent, ViewportId};
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    /// App with a valid saved root, Setup tab open, path check finished.
    fn harness(data: &std::path::Path, root: &std::path::Path) -> Harness<'static, FilerApp> {
        let settings = Settings {
            root: Some(root.to_path_buf()),
            ..Settings::default()
        };
        settings.save(data).unwrap();
        let data = data.to_path_buf();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1200.0, 1000.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(data, settings, true, None))
            });
        h.state_mut().tab = Tab::Setup;
        wait_checked(&mut h);
        h
    }

    /// V1, V2: the header button cycles the theme, zoom from the keys is kept in range, and
    /// both are saved at once without touching Setup's draft.
    #[test]
    fn theme_and_zoom_apply_and_save() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, root) = (tmp.path().join("data"), tmp.path().join("root"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let mut h = harness(&data, &root);
        let saved = || Settings::load_or_default(&data).0.display;
        h.get_by_label("Theme: System").click();
        h.step();
        h.step();
        assert_eq!(saved().theme, crate::config::ThemeChoice::Light);
        assert_eq!(
            h.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::Light
        );
        h.get_by_label("Theme: Light").click();
        h.step();
        h.step();
        assert_eq!(saved().theme, crate::config::ThemeChoice::Dark);

        // The real keys (egui's built-in zoom shortcuts).
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Plus);
        h.step();
        h.step();
        assert!(saved().zoom > 1.0, "Ctrl+Plus saved {}", saved().zoom);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num0);
        h.step();
        h.step();
        assert_eq!(saved().zoom, 1.0);
        // Ctrl+Plus changes the context's zoom; it is clamped and saved.
        h.ctx.set_zoom_factor(1.2);
        h.step();
        h.step();
        assert_eq!(saved().zoom, 1.2);
        h.ctx.set_zoom_factor(4.0);
        h.step();
        h.step();
        assert_eq!(h.ctx.zoom_factor(), 1.5);
        assert_eq!(saved().zoom, 1.5);
        assert!(
            !h.state().setup.is_dirty(&h.state().shared),
            "Setup has no edits"
        );
    }

    fn wait_checked(h: &mut Harness<'static, FilerApp>) {
        for _ in 0..200 {
            h.step();
            if h.state().setup.can_save() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("path check never finished");
    }

    fn edit_id_pattern(h: &mut Harness<'static, FilerApp>, extra: &str) {
        h.get_by_label("ID PATTERN").click();
        h.step();
        h.get_by_label("ID PATTERN").type_text(extra);
        h.step();
        wait_checked(h);
    }

    /// Simulates the title-bar X; returns the viewport commands from that frame.
    fn request_close(h: &mut Harness<'static, FilerApp>) -> Vec<ViewportCommand> {
        h.input_mut()
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .events
            .push(ViewportEvent::Close);
        h.step();
        root_commands(h)
    }

    fn root_commands(h: &Harness<'static, FilerApp>) -> Vec<ViewportCommand> {
        h.output()
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    fn click_until_closed(h: &mut Harness<'static, FilerApp>, label: &str) -> bool {
        // The prompt is drawn last, so its button is the last match (Setup has a Save too).
        h.get_all_by_label(label).last().unwrap().click();
        for _ in 0..5 {
            h.step();
            if root_commands(h)
                .iter()
                .any(|c| matches!(c, ViewportCommand::Close))
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn close_without_edits_is_not_blocked() {
        let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut h = harness(data.path(), root.path());
        let cmds = request_close(&mut h);
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, ViewportCommand::CancelClose))
        );
        assert!(!h.state().close_prompt);
    }

    #[test]
    fn close_with_edits_prompts_and_discard_closes() {
        let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut h = harness(data.path(), root.path());
        edit_id_pattern(&mut h, "7");
        assert!(h.state().setup.is_dirty(&h.state().shared));

        let cmds = request_close(&mut h);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, ViewportCommand::CancelClose))
        );
        h.step();
        h.get_by_label("Unsaved changes");

        assert!(click_until_closed(&mut h, "Discard"));
        let (saved, _) = Settings::load_or_default(data.path());
        assert_eq!(saved.id_pattern, crate::parse::DEFAULT_ID_PATTERN);
    }

    #[test]
    fn save_in_close_prompt_writes_then_closes() {
        let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut h = harness(data.path(), root.path());
        edit_id_pattern(&mut h, "7");
        request_close(&mut h);
        h.step();

        assert!(click_until_closed(&mut h, "Save"));
        let (saved, _) = Settings::load_or_default(data.path());
        assert_eq!(
            saved.id_pattern,
            format!("{}7", crate::parse::DEFAULT_ID_PATTERN)
        );
    }

    #[test]
    fn cancel_keeps_window_and_edits() {
        let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut h = harness(data.path(), root.path());
        edit_id_pattern(&mut h, "7");
        request_close(&mut h);
        h.step();
        h.get_by_label("Cancel").click();
        h.step();
        h.step();
        assert!(!h.state().close_prompt);
        assert!(!h.state().allow_close);
        assert!(h.state().setup.is_dirty(&h.state().shared));
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn harness(data: &std::path::Path, root: &std::path::Path) -> Harness<'static, FilerApp> {
        harness_with(data, root, true)
    }

    /// `savable = false` starts the app as if config.json existed but could not be read.
    fn harness_with(
        data: &std::path::Path,
        root: &std::path::Path,
        savable: bool,
    ) -> Harness<'static, FilerApp> {
        let settings = Settings {
            root: Some(root.to_path_buf()),
            ..Settings::default()
        };
        if savable {
            settings.save(data).unwrap();
        } else {
            std::fs::write(Settings::path(data), "{ broken").unwrap();
        }
        let data = data.to_path_buf();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1100.0, 2100.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(data, settings, savable, None))
            });
        h.state_mut().tab = Tab::Setup;
        settle(&mut h, |h| !h.state().shared.clients.busy());
        h
    }

    /// Steps frames (letting worker threads finish) until `done` holds.
    fn settle(
        h: &mut Harness<'static, FilerApp>,
        done: impl Fn(&Harness<'static, FilerApp>) -> bool,
    ) {
        for _ in 0..300 {
            h.step();
            if done(h) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("condition never reached");
    }

    fn open_wizard(h: &mut Harness<'static, FilerApp>, csv: &std::path::Path) {
        let ctx = h.ctx.clone();
        let app = h.state_mut();
        app.setup
            .open_wizard(&ctx, csv.to_path_buf(), &app.shared.settings);
        settle(h, |h| {
            h.query_by(|n| {
                n.label()
                    .is_some_and(|l| l.starts_with("Import ") && l.contains("client"))
            })
            .is_some()
        });
    }

    #[test]
    fn wizard_imports_csv_and_records_mapping() {
        let (data, root, src) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let csv = src.path().join("clients.csv");
        std::fs::write(
            &csv,
            "Client ID,Client Name,Partner\n1001,\"Smith, John\",KW\n,Nobody,KW\n1002,Acme LLC,AB\n",
        )
        .unwrap();
        let mut h = harness(data.path(), root.path());
        open_wizard(&mut h, &csv);

        // ID and name were guessed from the headers; tick the extra column.
        h.get_by_label("Partner → {Partner}").click();
        h.step();
        h.step();
        crate::app::snapshot(&mut h, "csv_wizard");
        h.get_by_label("Import 2 clients").click();
        settle(&mut h, |h| h.state().shared.settings.csv_path.is_some());
        h.step();
        assert!(
            h.query_by_label("Import 2 clients").is_none(),
            "wizard closes once saved"
        );

        let app = h.state();
        let list = app.shared.clients.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.skipped_rows, 1);
        assert_eq!(list.match_by_id("1001").unwrap().extra["Partner"], "KW");
        assert!(!app.setup.is_dirty(&app.shared));

        let (saved, _) = Settings::load_or_default(data.path());
        assert_eq!(saved.csv_path.as_deref(), Some(csv.as_path()));
        assert_eq!(saved.column_map.id.as_deref(), Some("Client ID"));
        assert_eq!(saved.column_map.extras, vec!["Partner".to_string()]);
        let cached = crate::clients::load_cache(data.path()).unwrap().unwrap();
        assert_eq!(cached.clients.len(), 2);
    }

    #[test]
    fn wizard_shows_duplicate_ids_and_imports_nothing() {
        let (data, root, src) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let csv = src.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1001,A\n1001,B\n").unwrap();
        let mut h = harness(data.path(), root.path());
        open_wizard(&mut h, &csv);

        // Duplicates replace the preview and block Import (UI_SPEC §6).
        h.get_by_label_contains("used by more than one row, so nothing can be imported: 1001");
        h.get_by_label("1 duplicate");
        h.get_by_label("Import 2 clients").click();
        h.step();
        h.step();
        assert!(h.state().setup.wizard_open());
        assert!(h.state().shared.clients.list().is_none());
        assert!(h.state().shared.settings.csv_path.is_none());
        assert!(!crate::clients::cache_path(data.path()).exists());
    }

    #[test]
    fn import_with_unreadable_config_keeps_csv_as_unsaved_edit() {
        let (data, root, src) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let csv = src.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,A
",
        )
        .unwrap();
        let mut h = harness_with(data.path(), root.path(), false);
        open_wizard(&mut h, &csv);
        h.get_by_label("Import 1 client").click();
        settle(&mut h, |h| h.state().shared.clients.list().is_some());
        h.step();

        // Not written over the unreadable file, not forgotten either.
        assert_eq!(
            std::fs::read_to_string(Settings::path(data.path())).unwrap(),
            "{ broken"
        );
        assert!(h.state().shared.settings.csv_path.is_none());
        assert!(h.state().setup.is_dirty(&h.state().shared));
        h.get_by_label_contains("could not be saved to config.json");

        // Save in Setup stores it (and sets the unreadable file aside).
        settle(&mut h, |h| h.state().setup.can_save());
        h.get_by_label("Save").click();
        settle(&mut h, |h| h.state().shared.settings.csv_path.is_some());
        let (saved, ok) = Settings::load_or_default(data.path());
        assert!(ok);
        assert_eq!(saved.csv_path.as_deref(), Some(csv.as_path()));
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use crate::config::ColumnMap;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn settle(
        h: &mut Harness<'static, FilerApp>,
        done: impl Fn(&Harness<'static, FilerApp>) -> bool,
    ) {
        for _ in 0..300 {
            h.step();
            if done(h) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("condition never reached");
    }

    #[test]
    fn rescan_bind_and_ignore_from_conflicts_tab() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&data).unwrap();
        let acme = root.join("Acme LLC (1002)");
        let junk = root.join("Junk");
        std::fs::create_dir_all(&acme).unwrap();
        std::fs::create_dir_all(&junk).unwrap();
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1002,Acme LLC\n").unwrap();
        let settings = Settings {
            root: Some(root.clone()),
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        settings.save(&data).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1100.0, 900.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, |h| h.state().shared.clients.list().is_some());

        h.get_by_label("Conflicts").click();
        h.step();
        h.get_by_label_contains("not been scanned yet");
        h.get_by_label("Rescan root").click();
        settle(&mut h, |h| {
            h.state().shared.folders.scan_of(Some(&root)).is_some()
        });
        h.step();

        h.get_by_label("Conflicts: 2 unresolved");
        h.get_by_label("Unbound · named");
        h.get_by_label("ID match");
        crate::app::snapshot(&mut h, "conflicts_tab");
        h.get_by_label("Bind · write sentinel").click();
        settle(&mut h, |h| {
            h.state()
                .shared
                .folders
                .bound_folder(Some(&root), "1002")
                .is_some()
        });
        let s = crate::sentinel::read_sentinel(&acme).unwrap().unwrap();
        assert_eq!(s.tax_software_id, "1002");
        let st = &h.state().shared;
        assert_eq!(Some(s.firm_id), st.settings.firm_id);
        assert_eq!(
            s.client_id,
            st.clients.list().unwrap().match_by_id("1002").unwrap().uuid
        );
        assert_eq!(s.display_name_at_creation, "Acme LLC");
        let ops = crate::oplog::read(&crate::oplog::path(&data))
            .unwrap()
            .records;
        assert!(
            ops.iter()
                .any(|r| r.op == crate::oplog::Op::Sentinel
                    && r.client_id.as_deref() == Some("1002"))
        );
        h.step();

        // Only Junk is left; ignore it.
        h.get_by_label("Ignore").click();
        h.step();
        h.step();
        h.get_by_label("Show ignored (1)");
        assert!(h.query_by_label_contains("Conflicts:").is_none());
        assert_eq!(h.state().shared.conflict_count(), 0);
        h.get_by_label_contains("Every folder is bound");
        let folders = std::fs::read_to_string(data.join(crate::sentinel::FOLDERS_FILE)).unwrap();
        let file: crate::sentinel::FoldersFile = serde_json::from_str(&folders).unwrap();
        assert!(
            file.ignored.iter().any(|p| p.ends_with("Junk")),
            "{folders}"
        );
    }

    /// V9: folders named for a client without "(ID)" are bound in one go, renamed to the
    /// template's name, and Undo renames them back.
    #[test]
    fn bind_all_rename_to_template_and_undo() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&data).unwrap();
        for n in ["Smith, John", "Acme LLC (1002)", "Baker Farm"] {
            std::fs::create_dir_all(root.join(n)).unwrap();
        }
        std::fs::write(root.join("Smith, John").join("old.pdf"), b"x").unwrap();
        // The renamed Acme name is already taken by a file.
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
1002,Acme LLC
1004,Baker Farms
",
        )
        .unwrap();
        let settings = Settings {
            root: Some(root.clone()),
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        settings.save(&data).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1400.0, 900.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, |h| h.state().shared.clients.list().is_some());
        h.get_by_label("Conflicts").click();
        h.step();
        h.get_by_label("Rescan root").click();
        settle(&mut h, |h| {
            h.state().shared.folders.scan_of(Some(&root)).is_some()
        });
        h.step();
        h.step();

        // Smith (exact name) and Acme (ID); Baker Farm is only close.
        h.get_by_label("Bind all exact name matches (2)").click();
        h.step();
        h.step();
        h.get_by_label("Bind 2 folders").click();
        settle(&mut h, |h| {
            let f = &h.state().shared.folders;
            f.bound_folder(Some(&root), "1001").is_some()
                && f.bound_folder(Some(&root), "1002").is_some()
        });
        assert!(
            h.state()
                .shared
                .folders
                .bound_folder(Some(&root), "1004")
                .is_none()
        );
        h.step();
        h.step();

        h.get_by_label("Rename to template name (1)…").click();
        h.step();
        h.step();
        crate::app::snapshot(&mut h, "rename_folders");
        h.get_by_label("Rename 1 folder").click();
        let new = root.join("Smith, John (1001)");
        settle(&mut h, |h| {
            h.state().shared.folders.bound_folder(Some(&root), "1001") == Some(new.as_path())
        });
        assert!(new.join("old.pdf").is_file());
        assert!(!root.join("Smith, John").exists());
        let lb = h.state().shared.last_batch.clone().unwrap();
        assert_eq!(lb.renamed_dirs.len(), 1);

        h.get_by_label("History").click();
        h.step();
        h.step();
        h.get_by_label("Undo last batch…").click();
        h.step();
        h.step();
        h.get_by_label("Confirm undo").click();
        settle(&mut h, |h| {
            h.state()
                .shared
                .last_batch
                .as_ref()
                .is_some_and(|b| b.undone)
        });
        h.step();
        assert!(root.join("Smith, John").join("old.pdf").is_file());
        assert!(!new.exists());
        assert_eq!(
            h.state().shared.folders.bound_folder(Some(&root), "1001"),
            Some(root.join("Smith, John").as_path())
        );
    }
}

#[cfg(test)]
mod rules_tests {
    use super::*;
    use egui::{ViewportCommand, ViewportEvent, ViewportId};
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn harness(data: &std::path::Path) -> Harness<'static, FilerApp> {
        let d = data.to_path_buf();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1300.0, 1400.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(
                    &cc.egui_ctx,
                    AppState::load(d, Settings::default(), true, None),
                )
            });
        h.step();
        h.get_by_label("Rules").click();
        h.step();
        h
    }

    fn names(h: &Harness<'static, FilerApp>) -> Vec<String> {
        h.state()
            .rules
            .draft_rules()
            .iter()
            .map(|r| r.name.clone())
            .collect()
    }

    /// V10: words of the test name build a rule, which lands above Catch-all.
    #[test]
    fn rule_builder_creates_a_rule() {
        let data = tempfile::tempdir().unwrap();
        let mut h = harness(data.path());
        let input = "Test a filename";
        h.get_by_label(input).click();
        h.step();
        h.get_by_label(input)
            .type_text("Smith, John_1234_2025 Bank Statement draft.pdf");
        h.step();
        h.step();
        h.get_by_label("All of").click();
        h.step();
        h.get_by_label("Word: Bank").click();
        h.step();
        h.get_by_label("Word: Statement").click();
        h.step();
        h.get_by_label("Not").click();
        h.step();
        h.get_by_label("Word: draft").click();
        h.step();
        h.step();
        h.get_by_label_contains("The test name does not match.");
        h.get_by_label("Remove draft from Does not contain").click();
        h.step();
        h.step();
        h.get_by_label_contains("The test name matches.");
        // The card runs below the window: make it taller.
        h.set_size(egui::vec2(1300.0, 2000.0));
        h.step();
        h.step();
        crate::app::snapshot(&mut h, "rule_builder");
        h.get_by_label("Create rule").click();
        h.step();
        h.step();
        let rules = h.state().rules.draft_rules();
        let n = rules.len();
        let made = &rules[n - 2];
        assert_eq!(made.name, "Bank");
        assert!(rules[n - 1].catch_all);
        let compiled = crate::rules::CompiledRules::new(rules).unwrap();
        assert_eq!(
            compiled.first_match("2024 statement_bank", false).name,
            "Bank"
        );
        assert_eq!(compiled.first_match("bank 2024", false).name, "Catch-all");
    }

    /// V11: an install with saved rules gets the Firm rules as an extra profile.
    #[test]
    fn add_default_profile_keeps_the_others() {
        let data = tempfile::tempdir().unwrap();
        let mut h = harness(data.path());
        assert_eq!(names(&h)[0], "K-1 package");
        h.get_by_label("Add default profile").click();
        h.step();
        h.step();
        assert_eq!(names(&h)[5], "Payroll");
        h.get_by_label("Add default profile").click();
        h.step();
        let profiles: Vec<String> = h.state().rules.draft_profiles();
        assert_eq!(profiles, ["Firm", "Firm 2", "Tax"]);
    }

    /// The Delete rule button removes the selected default rule; Catch-all can't be deleted.
    #[test]
    fn delete_rule_button_removes_a_default_rule() {
        let data = tempfile::tempdir().unwrap();
        let mut h = harness(data.path());
        h.get_by_label("Add default profile").click();
        h.step();
        h.step();
        let before = names(&h);
        assert_eq!(before[5], "Payroll");
        h.get_by_label("Payroll").click();
        h.step();
        h.get_by_label("Delete rule").click();
        h.step();
        h.step();
        let after = names(&h);
        assert_eq!(after.len(), before.len() - 1);
        assert!(!after.iter().any(|n| n == "Payroll"));
        h.get_by_label("Catch-all").click();
        h.step();
        {
            use egui_kittest::kittest::NodeT;
            assert!(h.get_by_label("Delete rule").accesskit_node().is_disabled());
        }
        h.get_by_label("Save").click();
        h.step();
        h.step();
        let (saved, _, _) = crate::rules::load_or_seed(data.path());
        assert!(!saved.profiles["Firm"].iter().any(|r| r.name == "Payroll"));
    }

    #[test]
    fn test_box_reorder_save_and_reload() {
        let data = tempfile::tempdir().unwrap();
        let mut h = harness(data.path());

        // Test box: full filename and bare remainder.
        let input = "Test a filename";
        h.get_by_label(input).click();
        h.step();
        h.get_by_label(input)
            .type_text("Smith, John_1234_2025_1040.pdf");
        h.step();
        assert_eq!(
            h.get_all_by_label("Tax Return").count(),
            2,
            "list row and result"
        );
        h.get_by_label("(5th tried)");
        h.get_by_label("2025 Tax Return - Sample Client.pdf");
        crate::app::snapshot(&mut h, "rules_tab");

        // Reorder: move K-1 package (first) down below K-1.
        assert_eq!(names(&h)[..2], ["K-1 package", "K-1"]);
        h.get_by_label("K-1 package").click_secondary();
        h.step();
        h.step();
        h.get_by_label("Move down").click();
        h.step();
        assert_eq!(names(&h)[..2], ["K-1", "K-1 package"]);
        assert!(h.state().rules.is_dirty(&h.state().shared));

        h.get_by_label("Save").click();
        h.step();
        assert!(!h.state().rules.is_dirty(&h.state().shared));
        let (reloaded, ok, _) = crate::rules::load_or_seed(data.path());
        assert!(ok);
        let order: Vec<&str> = reloaded
            .active_rules()
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(order[..2], ["K-1", "K-1 package"]);
        assert!(reloaded.active_rules().last().unwrap().catch_all);
    }

    #[test]
    fn unsaved_rules_trigger_close_prompt() {
        let data = tempfile::tempdir().unwrap();
        let mut h = harness(data.path());
        h.get_by_label("Enabled: K-1").click();
        h.step();
        assert!(h.state().rules.is_dirty(&h.state().shared));

        h.input_mut()
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .events
            .push(ViewportEvent::Close);
        h.step();
        let cmds = h
            .output()
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, ViewportCommand::CancelClose))
        );
        h.step();
        h.get_by_label_contains("Save your Rules changes");
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use egui_kittest::HarnessBuilder;
    use egui_kittest::kittest::Queryable;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestDrop(PathBuf);

    impl egui::DroppedFile for TestDrop {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn dropped_files_open_preview_and_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let desk = tmp.path().join("desk");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&desk).unwrap();
        let pdf = desk.join("Smith, John_1234_2025_1040.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 test").unwrap();
        std::fs::write(desk.join("notes.xyz"), b"xyz").unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1100.0, 700.0])
            .build_eframe(move |cc| {
                FilerApp::new(
                    &cc.egui_ctx,
                    AppState::load(d, Settings::default(), true, None),
                )
            });
        h.step();
        h.input_mut()
            .dropped_files
            .push(Arc::new(TestDrop(desk.clone())));
        h.step();
        assert_eq!(h.state().tab, Tab::Preview);
        for _ in 0..300 {
            h.step();
            // Rows appear once the background plan for the queued file is built.
            if h.state().shared.intake.queue().len() == 1
                && h.state().shared.intake.unsupported().len() == 1
                && h.state().preview.proposals().len() == 1
                && h.state().preview.plan_idle()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        h.step();
        h.get_by_label("Smith, John_1234_2025_1040.pdf");
        h.get_by_label("Preview: 1 pending");
        h.get_by_label_contains("Unsupported (1)");
        crate::app::snapshot(&mut h, "preview_tab");
    }
}

#[cfg(test)]
mod plan_ui_tests {
    use super::*;
    use crate::config::ColumnMap;
    use crate::plan::{FolderAction, Proposal, RowStatus};
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    const FILES: [&str; 20] = [
        "Smith, John_1001_2025_1040.pdf",
        "Smith, John_1001_2025 organizer.pdf",
        "Smith, John_1001_2025 K-1 ABC Partners.pdf",
        "Smith, John_1001_EL 2025.pdf",
        "Smith, John_1001_bank statement.pdf",
        "Acme LLC_1002_2025_1120-S.pdf",
        "Acme LLC_1002_2025 payroll.pdf",
        "Acme LLC_1002_misc.pdf",
        "Baker Farms_1004_2025_1065.pdf",
        "Baker Farms_1004_2024 K-1.pdf",
        "Jones, Mary_1003_2025_1040.pdf",
        "Jonez Mary_9999_2025_1040.pdf",
        "scan0001.pdf",
        "Delta Corp_1005_2025 1040-ES.pdf",
        "Delta Corp_1005_2025 return.pdf",
        "Unknown Person_7777_2025_1040.pdf",
        "Baker Farms_1004_2025-1065.pdf",
        "Smith, John_1001_2025 1040.pdf",
        "Acme LLC_1002_2025 engagement.pdf",
        "Jones, Mary_1003_2025 organizer.pdf",
    ];

    fn settle(h: &mut Harness<'static, FilerApp>, what: &str, done: impl Fn(&FilerApp) -> bool) {
        for _ in 0..500 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never reached: {what}");
    }

    fn proposal(app: &FilerApp, name: &str) -> Proposal {
        app.preview
            .proposals()
            .iter()
            .find(|p| p.file.path.file_name().unwrap() == name)
            .unwrap_or_else(|| panic!("no row for {name}"))
            .clone()
    }

    fn status(app: &FilerApp, name: &str) -> RowStatus {
        proposal(app, name).status
    }

    /// Position of `name` among the rows `filter` keeps, in table order.
    fn row_index(app: &FilerApp, name: &str, filter: impl Fn(&Proposal) -> bool) -> usize {
        app.preview
            .proposals()
            .iter()
            .filter(|p| filter(p))
            .position(|p| p.file.path.file_name().unwrap() == name)
            .unwrap()
    }

    fn idle_and(app: &FilerApp, name: &str, want: RowStatus) -> bool {
        app.preview.plan_idle() && status(app, name) == want
    }

    #[test]
    fn twenty_file_batch_resolves_to_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name\n1001,\"Smith, John\"\n1002,Acme LLC\n1003,\"Jones, Mary\"\n1004,Baker Farms\n1005,Delta Corp\n",
        )
        .unwrap();
        // Smith and Jones are bound; Acme has an ID-named folder without a sentinel; Baker and
        // Delta have no folder.
        for (folder, id, name) in [
            ("Smith, John (1001)", "1001", "Smith, John"),
            ("Jones (1003)", "1003", "Jones, Mary"),
        ] {
            let d = root.join(folder);
            std::fs::create_dir_all(&d).unwrap();
            let c = crate::clients::Client {
                uuid: uuid::Uuid::new_v4(),
                id: id.into(),
                name: name.into(),
                extra: Default::default(),
            };
            crate::sentinel::write_sentinel(&root, &d, &c, uuid::Uuid::new_v4()).unwrap();
        }
        std::fs::create_dir_all(root.join("Acme LLC (1002)")).unwrap();
        let existing = root.join("Jones (1003)").join("Tax Returns").join("2025");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("2025 Tax Return - Jones, Mary.pdf"), b"old").unwrap();
        for f in FILES {
            std::fs::write(inbox.join(f), b"%PDF-1.4 fixture").unwrap();
        }

        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1600.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue and clients", |a| {
            a.shared.intake.queue().len() == 20 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview: 20 pending");
        h.get_by_label("Preview").click();
        settle(&mut h, "plan after the rescan", |a| {
            a.preview.plan_idle()
                && a.preview.proposals().len() == 20
                && !a
                    .preview
                    .proposals()
                    .iter()
                    .any(|p| matches!(p.status, crate::plan::RowStatus::Pending(_)))
        });

        // Proposals before any fixes.
        let app = h.state();
        assert_eq!(
            status(app, "Smith, John_1001_2025_1040.pdf"),
            RowStatus::Ready
        );
        assert_eq!(
            status(app, "Smith, John_1001_bank statement.pdf"),
            RowStatus::NeedsYear
        );
        assert_eq!(status(app, "Acme LLC_1002_misc.pdf"), RowStatus::NeedsYear);
        assert_eq!(
            status(app, "Jonez Mary_9999_2025_1040.pdf"),
            RowStatus::NeedsClient
        );
        assert_eq!(status(app, "scan0001.pdf"), RowStatus::NeedsClient);
        assert_eq!(
            status(app, "Unknown Person_7777_2025_1040.pdf"),
            RowStatus::NeedsClient
        );
        assert_eq!(
            proposal(app, "Smith, John_1001_2025 K-1 ABC Partners.pdf").rule_name,
            "K-1"
        );
        assert_eq!(
            proposal(app, "Smith, John_1001_EL 2025.pdf").rule_name,
            "Engagement Letter"
        );
        assert_eq!(
            proposal(app, "Delta Corp_1005_2025 1040-ES.pdf").rule_name,
            "Catch-all"
        );
        assert!(matches!(
            proposal(app, "Acme LLC_1002_2025_1120-S.pdf").folder,
            Some(FolderAction::WillBind(_))
        ));
        assert!(matches!(
            proposal(app, "Baker Farms_1004_2025_1065.pdf").folder,
            Some(FolderAction::WillCreate(_))
        ));
        assert!(
            proposal(app, "Jones, Mary_1003_2025_1040.pdf").renamed,
            "file already on disk"
        );
        let baker_renamed = proposal(app, "Baker Farms_1004_2025-1065.pdf").renamed
            || proposal(app, "Baker Farms_1004_2025_1065.pdf").renamed;
        assert!(baker_renamed, "two Baker 1065s in one batch");
        assert!(!app.preview.can_commit_for_test());
        crate::app::snapshot(&mut h, "preview_before");

        // 1. Batch year 2024 resolves the rows without a year.
        h.get_by_role_and_label(Role::ComboBox, "Batch year")
            .click();
        h.step();
        h.step();
        h.step();
        h.get_by_label("2024").click();
        settle(&mut h, "batch year", |a| {
            idle_and(a, "Acme LLC_1002_misc.pdf", RowStatus::Ready)
        });
        assert_eq!(
            status(h.state(), "Smith, John_1001_bank statement.pdf"),
            RowStatus::Ready
        );

        // 2. Accept the name suggestion for "Jonez Mary".
        h.get_by_label_contains("Use Jones, Mary (1003)").click();
        settle(&mut h, "suggestion", |a| {
            idle_and(a, "Jonez Mary_9999_2025_1040.pdf", RowStatus::Ready)
        });

        // 3. Pick a client for scan0001 in the picker.
        let i = row_index(h.state(), "scan0001.pdf", |p| p.client.is_none());
        // Grid columns are sized from the previous frame; let them settle before clicking.
        for _ in 0..3 {
            h.step();
        }
        h.get_all_by_label("Choose client…")
            .nth(i)
            .unwrap()
            .scroll_to_me();
        for _ in 0..3 {
            h.step();
        }
        h.get_all_by_label("Choose client…").nth(i).unwrap().click();
        settle(&mut h, "picker open", |a| a.preview.picker_open());
        h.step();
        h.get_by_label("Search by name or ID").click();
        h.step();
        // The search starts with the file name; replace it.
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.step();
        h.get_by_label("Search by name or ID").type_text("1005");
        h.step();
        h.step();
        h.get_by_label("Delta Corp (1005)  ID match").click();
        settle(&mut h, "picked client", |a| {
            idle_and(a, "scan0001.pdf", RowStatus::Ready)
        });

        // 4. Skip the unknown client.
        for _ in 0..3 {
            h.step();
        }
        // Skip is in the row's right-click menu (X125).
        let unknown = "Unknown Person_7777_2025_1040.pdf";
        h.get_by_label(unknown).scroll_to_me();
        h.step();
        h.get_by_label(unknown).click_secondary();
        h.step();
        h.step();
        h.get_by_label("Skip row").click();
        settle(&mut h, "skipped", |a| {
            idle_and(a, "Unknown Person_7777_2025_1040.pdf", RowStatus::Skipped)
        });

        // 5. The K-1 names "ABC Partners", not one of Smith's relationships or clients (P07a):
        //    file it without the copy.
        let k1 = "Smith, John_1001_2025 K-1 ABC Partners.pdf";
        h.get_by_label("Remove copy: ABC Partners").scroll_to_me();
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Remove copy: ABC Partners").click();
        settle(&mut h, "copy removed", |a| {
            idle_and(a, k1, RowStatus::Ready)
        });

        // Every row is Ready or Skipped; Commit opens the summary.
        assert!(
            h.state()
                .preview
                .proposals()
                .iter()
                .all(|p| !p.status.blocks_commit())
        );
        crate::app::snapshot(&mut h, "preview_resolved");
        h.get_by_label("Commit 19 files").click();
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        h.step();
        // UI_SPEC §4: count tiles and one line per category.
        let sum = crate::plan::summarize(h.state().preview.proposals());
        assert_eq!(
            (
                sum.files,
                sum.moves,
                sum.new_client_folders,
                sum.binds,
                sum.skipped
            ),
            (19, 19, 2, 1, 1)
        );
        // Baker (2), Smith (2), and Jones (2) + (3): the "Jonez" file became a Jones return too.
        assert_eq!(sum.renamed, 4);
        h.get_by_label("Commit 19 files?");
        h.get_by_label("moves (same volume)");
        h.get_by_label("overwrites · 4 kept both");
        // Also pills on the rows behind the modal.
        assert!(h.get_all_by_label("Will create").count() > 4);
        assert!(h.get_all_by_label("Will bind").count() > 4);
        h.get_by_label("Kept both");
        h.get_by_label("1 row");
        crate::app::snapshot(&mut h, "preview_summary");
        // P07: Confirm files the batch.
        let root_before = tree(&root);
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Confirm commit").click();
        settle(&mut h, "filed", |a| {
            a.shared.ops_generation > 0 && !a.preview.committing()
        });
        h.step();
        h.get_by_label_contains("Filed 19 files.");
        let left: Vec<_> = std::fs::read_dir(&inbox).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "only the skipped file stays in the inbox");
        assert!(root.join("Baker Farms (1004)").join("Permanent").is_dir());
        assert!(
            crate::sentinel::read_sentinel(&root.join("Acme LLC (1002)"))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            std::fs::read(existing.join("2025 Tax Return - Jones, Mary.pdf")).unwrap(),
            b"old",
            "keep both: the existing return is untouched"
        );
        let lb = h.state().shared.last_batch.clone().unwrap();
        assert_eq!(lb.items.len(), 19);

        // History lists the batch; Undo puts everything back.
        h.get_by_label("History").click();
        settle(&mut h, "history loaded", |a| !a.history.loading());
        h.step();
        assert!(h.get_all_by_label("move").count() >= 19);
        crate::app::snapshot(&mut h, "history_tab");
        h.get_by_label("Undo last batch…").click();
        h.step();
        h.step();
        h.get_by_label("19 files go back to their inbox");
        h.get_by_label("Confirm undo").click();
        settle(&mut h, "undone", |a| {
            a.shared.last_batch.as_ref().is_some_and(|b| b.undone)
        });
        h.step();
        h.get_by_label_contains("Undone: 19 files back.");
        assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 20);
        assert_eq!(
            tree(&root),
            root_before,
            "root exactly as before the commit"
        );
    }

    /// Every path under `dir` with its file content.
    fn tree(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        fn walk(
            base: &std::path::Path,
            d: &std::path::Path,
            out: &mut std::collections::BTreeMap<String, Vec<u8>>,
        ) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(base).unwrap().display().to_string();
                if p.is_dir() {
                    out.insert(format!("{rel}/"), Vec::new());
                    walk(base, &p, out);
                } else {
                    out.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        walk(dir, dir, &mut out);
        out
    }
}

#[cfg(test)]
mod combine_ui_tests {
    use super::*;
    use crate::config::ColumnMap;
    use crate::plan::RowStatus;
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn settle(h: &mut Harness<'static, FilerApp>, what: &str, done: impl Fn(&FilerApp) -> bool) {
        for _ in 0..500 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never reached: {what}");
    }

    fn plan_ready(a: &FilerApp, rows: usize) -> bool {
        a.preview.plan_idle()
            && a.preview.proposals().len() == rows
            && !a
                .preview
                .proposals()
                .iter()
                .any(|p| matches!(p.status, RowStatus::Pending(_)))
    }

    /// Ticks the "Select" box of the row for `name`, then waits for the re-plan ticks cause
    /// (ticked rows are numbered first).
    fn tick(h: &mut Harness<'static, FilerApp>, name: &str) {
        h.get_by_role_and_label(Role::CheckBox, &format!("Select {name}"))
            .click();
        // The click is applied on the next frame; give it two.
        h.step();
        h.step();
        settle(h, "plan after tick", |a| a.preview.plan_idle());
        h.step();
    }

    #[test]
    fn combine_commit_and_undo_through_the_ui() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1001,\"Smith, John\"\n1004,Baker Farms\n").unwrap();
        let baker = [
            "Baker Farms_1004_2025_1065.pdf",
            "Baker Farms_1004_2025 page2.pdf",
            "Baker Farms_1004_2025 page3.pdf",
        ];
        for (i, n) in baker.iter().enumerate() {
            let pages: Vec<String> = (0..=i).map(|k| format!("{n} p{k}")).collect();
            let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
            crate::pdf::write_test_pdf(&inbox.join(n), &refs);
        }
        let smith = "Smith, John_1001_2025_1040.pdf";
        crate::pdf::write_test_pdf(&inbox.join(smith), &["Smith return"]);

        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 4 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview: 4 pending");
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 4));

        // Ticking a different client's row blocks Combine.
        tick(&mut h, baker[0]);
        tick(&mut h, smith);
        assert!(h.state().preview.can_combine_for_test().is_some());
        tick(&mut h, smith);

        // Tick order is the start order (X86): 1065, then page2, page3.
        tick(&mut h, baker[1]);
        tick(&mut h, baker[2]);
        h.get_by_label("Combine").click();
        h.step();
        h.step();
        h.get_by_label("1. Baker Farms_1004_2025_1065.pdf");
        h.get_by_label("3. Baker Farms_1004_2025 page3.pdf");
        for _ in 0..3 {
            h.step();
        }
        crate::app::snapshot(&mut h, "combine_dialog");
        h.step();
        h.get_by_label("Combine · replaces 3 rows").click();
        settle(&mut h, "combined", |a| {
            a.preview.group_count() == 1 && !a.preview.combining()
        });
        settle(&mut h, "plan after combine", |a| plan_ready(a, 2));
        h.step();
        h.get_by_label("Combined: 3 files (6 pages)");
        crate::app::snapshot(&mut h, "preview_combined");

        h.get_by_label("Commit 2 files").click();
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        h.step();
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Confirm commit").click();
        settle(&mut h, "filed", |a| {
            a.shared.ops_generation > 0 && !a.preview.committing()
        });
        let filed = root
            .join("Baker Farms (1004)")
            .join("Tax Returns")
            .join("2025")
            .join("2025 Tax Return - Baker Farms.pdf");
        assert_eq!(crate::pdf::page_count(&filed).unwrap(), 6);
        let combined = inbox.join(crate::intake::COMBINED_DIR);
        for n in baker {
            assert!(combined.join(n).exists(), "{n} parked");
        }
        assert_eq!(h.state().preview.group_count(), 0);

        // Undo from History restores the originals.
        // Badges change as the queue empties; let the header settle before clicking.
        h.step();
        h.step();
        h.get_by_label("History").click();
        settle(&mut h, "history", |a| {
            a.tab == Tab::History && !a.history.loading()
        });
        h.step();
        h.get_by_label("Undo last batch…").click();
        h.step();
        h.step();
        h.get_by_label("Confirm undo").click();
        settle(&mut h, "undone", |a| {
            a.shared.last_batch.as_ref().is_some_and(|b| b.undone)
        });
        for n in baker.iter().chain([&smith]) {
            assert!(inbox.join(n).exists(), "{n} back in the inbox");
        }
        assert!(!combined.exists(), "_combined removed once empty");
        assert!(!filed.exists());
    }

    /// Every file under `dir`, recursively.
    fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    /// V25: a Word file is queued and filed as it is; a picture is filed as a PDF, the picture
    /// parked in `_combined`; Undo puts both back. Unlisted types stay unsupported.
    #[test]
    fn office_files_are_filed_and_images_become_pdfs() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1001,\"Smith, John\"\n1004,Baker Farms\n").unwrap();
        let word = "Smith, John_1001_2025 Questions.docx";
        let photo = "Baker Farms_1004_2025 Receipt.png";
        std::fs::write(inbox.join(word), b"PK\x03\x04 not really a docx").unwrap();
        image::RgbImage::from_pixel(80, 120, image::Rgb([240, 240, 240]))
            .save(inbox.join(photo))
            .unwrap();
        std::fs::write(inbox.join("notes.xyz"), b"?").unwrap();
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 2
                && a.shared.intake.unsupported().len() == 1
                && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "converted", |a| a.preview.group_count() == 1);
        settle(&mut h, "plan", |a| plan_ready(a, 2));
        h.step();
        let names: Vec<String> = h
            .state()
            .preview
            .proposals()
            .iter()
            .map(|p| crate::ui::theme::file_name(&p.file.path))
            .collect();
        assert!(names.contains(&word.to_string()), "{names:?}");
        assert!(
            names.contains(&"Baker Farms_1004_2025 Receipt.pdf".to_string()),
            "{names:?}"
        );
        h.get_by_label("Commit 2 files").click();
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        for _ in 0..4 {
            h.step();
        }
        h.get_by_label("Confirm commit").click();
        settle(&mut h, "filed", |a| {
            a.shared.ops_generation > 0 && !a.preview.committing()
        });
        let filed = files_under(&root);
        assert!(
            filed
                .iter()
                .any(|p| p.ends_with(word) || p.extension().is_some_and(|e| e == "docx")),
            "{filed:?}"
        );
        let pdf = filed
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "pdf"))
            .expect("the picture filed as a PDF");
        assert_eq!(crate::pdf::page_count(pdf).unwrap(), 1);
        assert!(
            !filed
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "png"))
        );
        let parked = inbox.join(crate::intake::COMBINED_DIR).join(photo);
        assert!(parked.exists(), "picture parked");
        assert!(inbox.join("notes.xyz").exists(), "unlisted type left alone");

        h.step();
        h.step();
        h.get_by_label("History").click();
        settle(&mut h, "history", |a| {
            a.tab == Tab::History && !a.history.loading()
        });
        h.step();
        h.get_by_label("Undo last batch…").click();
        h.step();
        h.step();
        h.get_by_label("Confirm undo").click();
        settle(&mut h, "undone", |a| {
            a.shared.last_batch.as_ref().is_some_and(|b| b.undone)
        });
        assert!(inbox.join(word).exists());
        assert!(inbox.join(photo).exists());
        assert!(!parked.exists());
        assert!(!pdf.exists());
    }

    /// V5, V6, V8: click the name to rename (extension kept), "Change folder…" from the row
    /// menu with a new subfolder, and "Change…" on a name-matched client.
    #[test]
    fn rename_change_folder_and_change_client() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
1004,Baker Farms
",
        )
        .unwrap();
        let names = [
            "Smith, John_1001_2025 1040.pdf",
            "Baker Farms_1004_notes.pdf",
            "Smith, Jon_9999_2025 W-2.pdf",
        ];
        for n in names {
            crate::pdf::write_test_pdf(&inbox.join(n), &[n]);
        }
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 3 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 3));
        let dest_of = |a: &FilerApp, n: &str| {
            a.preview
                .proposals()
                .iter()
                .find(|p| p.file.path.ends_with(n))
                .and_then(|p| p.dest.clone())
        };

        // V5: rename in place.
        h.get_by_label(&format!("Rename {}", names[0])).click();
        h.step();
        h.step();
        // The name opens selected: typing replaces it.
        h.get_by_label(&format!("New name for {}", names[0]))
            .type_text("Return.txt");
        h.step();
        h.key_press(egui::Key::Enter);
        settle(&mut h, "renamed", |a| {
            plan_ready(a, 3)
                && dest_of(a, names[0]).is_some_and(|d| d.file_name().unwrap() == "Return.txt.pdf")
        });

        // V6: the notes need a year; a picked folder with no year in the name doesn't.
        h.get_by_label(names[1]).click_secondary();
        h.step();
        h.step();
        h.get_by_label("Change folder…").click();
        h.step();
        h.step();
        h.get_by_label("Permanent").click();
        h.step();
        h.get_by_label_contains("type a new name to create it")
            .click();
        h.step();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.step();
        h.get_by_label_contains("type a new name to create it")
            .type_text("Permanent/Articles");
        h.step();
        h.step();
        crate::app::snapshot(&mut h, "folder_pick");
        h.get_by_label("Use this folder").click();
        settle(&mut h, "folder picked", |a| {
            plan_ready(a, 3)
                && dest_of(a, names[1]).is_some_and(|d| {
                    d.parent()
                        .unwrap()
                        .ends_with(std::path::Path::new("Permanent").join("Articles"))
                })
        });

        h.step();
        crate::app::snapshot(&mut h, "preview_renamed");
        // V8: the W-2 matched by name offers "Change…".
        h.get_by_label(&format!("Change client of {}", names[2]))
            .click();
        h.step();
        h.step();
        assert!(h.state().preview.picker_open());
    }

    /// V3: with rows ticked, Commit files only those; an unticked row that needs a client
    /// doesn't block it and stays queued, like the unticked Ready row.
    #[test]
    fn ticked_rows_commit_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1001,\"Smith, John\"\n1004,Baker Farms\n").unwrap();
        let names = [
            "Smith, John_1001_2025 1040.pdf",
            "Baker Farms_1004_2025 1065.pdf",
            "Nobody_9999_2025 W-2.pdf",
        ];
        for n in names {
            crate::pdf::write_test_pdf(&inbox.join(n), &[n]);
        }
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 3 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 3));
        assert!(!h.state().preview.can_commit_for_test(), "W-2 blocks");

        tick(&mut h, names[0]);
        // Ticks re-plan (ticked rows are numbered first).
        settle(&mut h, "plan after tick", |a| plan_ready(a, 3));
        h.step();
        assert!(h.state().preview.can_commit_for_test());
        h.get_by_label("Commit 1 selected").click();
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Confirm commit").click();
        settle(&mut h, "filed", |a| {
            a.shared.ops_generation > 0 && !a.preview.committing()
        });
        assert!(
            root.join("Smith, John (1001)")
                .join("Tax Returns")
                .join("2025")
                .join("2025 Tax Return - Smith, John.pdf")
                .exists()
        );
        assert!(!inbox.join(names[0]).exists());
        for n in &names[1..] {
            assert!(inbox.join(n).exists(), "{n} left in the inbox");
        }
        settle(&mut h, "plan after commit", |a| plan_ready(a, 2));
        assert!(
            !h.state().preview.can_commit_for_test(),
            "nothing ticked: W-2 blocks again"
        );
    }

    /// Host hooks (owner 2026-11-04, for Vibe Files): Setup locked by the host is read-only with
    /// the reason shown; with "New client…" offered, a row whose client is not found hands the
    /// host a request, and `client_created` gives the row that client.
    #[test]
    fn host_locks_setup_and_creates_a_missing_client() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
",
        )
        .unwrap();
        crate::pdf::write_test_pdf(&inbox.join("Newco LLC_2002_2025 W-2.pdf"), &["x"]);
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv.clone()),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 1000.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                let mut app = FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None));
                app.set_setup_lock(Some("Locked by the admin password.".into()));
                app.offer_new_client(true);
                app
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 1 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Setup").click();
        h.step();
        h.step();
        assert!(h.query_by_label("Locked by the admin password.").is_some());
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 1));
        h.step();
        let file = inbox.join("Newco LLC_2002_2025 W-2.pdf");
        h.get_by_label("New client for Newco LLC_2002_2025 W-2.pdf")
            .click();
        h.step();
        let req = h.state_mut().take_new_client_request().expect("asked");
        assert_eq!(req.file, file);
        assert_eq!(
            (req.name.as_str(), req.id.as_deref()),
            ("Newco LLC", Some("2002"))
        );
        // The host adds the client to clients.json (here: the CSV, re-imported), then says so.
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
2002,Newco LLC
",
        )
        .unwrap();
        let ctx = h.ctx.clone();
        h.state_mut().client_created(&ctx, &file, "2002");
        settle(&mut h, "client found", |a| {
            a.preview.plan_idle()
                && a.preview
                    .proposals()
                    .first()
                    .and_then(|p| p.client.as_ref())
                    .is_some_and(|c| c.id == "2002")
        });
    }

    /// Host hooks (owner 2026-11-05): "Move to trash" hands the host the files and they leave
    /// the queue once taken; rows no rule names by file name are listed for the host to read,
    /// and what it read fills only what the row lacks; long rule menus have a search box.
    #[test]
    fn host_trash_reading_and_rule_search() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
",
        )
        .unwrap();
        let scan = inbox.join("scan0001.pdf");
        let named = inbox.join("Smith, John_1001_2025 Organizer.pdf");
        crate::pdf::write_test_pdf(&scan, &["x"]);
        crate::pdf::write_test_pdf(&named, &["x"]);
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv.clone()),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 1000.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                let mut app = FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None));
                app.offer_trash(true);
                app
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 2 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 2));
        h.step();

        // Only the file whose name no rule names is offered for reading.
        assert_eq!(h.state().unnamed_files(), vec![scan.clone()]);
        // What the host read fills the client and year; a note shows on the row.
        h.state_mut().suggest(
            &scan,
            Some("1001"),
            Some(2024),
            "W-2 2024 read from the form",
        );
        settle(&mut h, "suggested", |a| {
            a.preview.plan_idle()
                && a.preview.proposals().iter().any(|p| {
                    p.file.path == scan
                        && p.client.as_ref().is_some_and(|c| c.id == "1001")
                        && p.year == Some(2024)
                })
        });
        assert!(h.query_by_label("read").is_some());
        // A row already named keeps what it has.
        h.state_mut().suggest(&named, None, Some(2019), "");
        settle(&mut h, "kept", |a| a.preview.plan_idle());
        assert!(
            h.state()
                .preview
                .proposals()
                .iter()
                .any(|p| p.file.path == named && p.year == Some(2025))
        );

        // The rule menu searches.
        let rule = h
            .state()
            .preview
            .proposals()
            .iter()
            .find(|p| p.file.path == named)
            .unwrap()
            .rule_name
            .clone();
        h.get_all_by_label(&rule).next().unwrap().click();
        h.step();
        h.step();
        assert!(h.query_by_label("Type to search").is_some(), "searchable");
        h.get_by_label("Type to search")
            .type_text("zzzz-no-such-rule");
        h.step();
        h.step();
        assert!(h.query_by_label_contains("0 of").is_some());
        h.key_press(egui::Key::Escape);
        h.step();
        h.step();
        // Enter with nothing typed keeps the row's rule.
        h.get_all_by_label(&rule).next().unwrap().click();
        h.step();
        h.step();
        h.get_by_label("Type to search").focus();
        h.step();
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        assert_eq!(
            h.state()
                .preview
                .proposals()
                .iter()
                .find(|p| p.file.path == named)
                .unwrap()
                .rule_name,
            rule
        );
        h.key_press(egui::Key::Escape);
        h.step();
        h.step();

        // Right-click › Move to trash hands the host the file; taken, it leaves the queue.
        h.get_by_label("scan0001.pdf").click_secondary();
        h.step();
        h.step();
        h.get_by_label("Move to trash").click();
        h.step();
        let asked = h.state_mut().take_trash_request().expect("asked");
        assert_eq!(asked, vec![scan.clone()]);
        std::fs::rename(&scan, tmp.path().join("trashed.pdf")).unwrap();
        h.state_mut().files_taken(&asked);
        settle(&mut h, "taken", |a| a.shared.intake.queue().len() == 1);
        assert!(h.state().shared.host_notes.is_empty());
    }

    /// Personal inboxes (owner 2026-11-05: several people on one Remote Desktop server): the
    /// host's folders are watched besides the shared ones, never saved into the shared
    /// settings, and Preview shows Shared inbox | My inbox, each tab its own batch.
    #[test]
    fn personal_inboxes_have_their_own_tab_and_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let shared = tmp.path().join("shared");
        let mine = tmp.path().join("mine");
        for d in [&data, &root, &shared, &mine] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1001,\"Smith, John\"\n").unwrap();
        crate::pdf::write_test_pdf(&shared.join("Smith, John_1001_2025 Organizer.pdf"), &["x"]);
        crate::pdf::write_test_pdf(&mine.join("scan0001.pdf"), &["x"]);
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![shared.clone()],
            csv_path: Some(csv.clone()),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let m = mine.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 1000.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                let mut app = FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None));
                app.set_personal_inboxes(vec![m]);
                app
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 2 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 1));
        h.step();
        assert!(h.query_by_label("Shared inbox (1)").is_some());
        assert!(h.query_by_label("My inbox (1)").is_some());
        let files = |h: &Harness<'_, FilerApp>| -> Vec<String> {
            h.state()
                .preview
                .proposals()
                .iter()
                .map(|p| crate::ui::theme::file_name(&p.file.path))
                .collect()
        };
        assert_eq!(files(&h), ["Smith, John_1001_2025 Organizer.pdf"]);
        h.get_by_label("My inbox (1)").click();
        settle(&mut h, "mine", |a| {
            a.preview.plan_idle()
                && a.preview.proposals().len() == 1
                && a.preview.proposals()[0].file.path.starts_with(&mine)
        });
        assert_eq!(files(&h), ["scan0001.pdf"]);
        // The personal inbox is never written into the shared settings.
        assert_eq!(h.state().shared.settings.inboxes, vec![shared.clone()]);
        // Taken away again: back to one queue.
        h.state_mut().set_personal_inboxes(vec![]);
        settle(&mut h, "shared only", |a| {
            a.shared.intake.queue().len() == 1
        });
        assert!(h.query_by_label("My inbox (0)").is_none());
    }

    /// V24: a double-click on a file name opens the file; a host that shows files itself gets it.
    #[test]
    fn double_click_on_a_file_name_hands_it_to_the_host() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        crate::pdf::write_test_pdf(&inbox.join("scan0001.pdf"), &["x"]);
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 1000.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                let mut app = FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None));
                app.offer_open(true);
                app
            });
        settle(&mut h, "queue", |a| a.shared.intake.queue().len() == 1);
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 1));
        h.step();
        // Both clicks in one frame (kittest's click() takes a frame per event).
        let pos = h.get_by_label("scan0001.pdf").rect().center();
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        for pressed in [true, false, true, false] {
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
        h.step();
        h.step();
        assert_eq!(
            h.state_mut().take_open_request(),
            Some(inbox.join("scan0001.pdf"))
        );
    }

    /// V23: Import has the same Shared inbox | My inbox tabs, each showing its own zips.
    #[test]
    fn import_has_shared_and_my_inbox_tabs() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let shared = tmp.path().join("shared");
        let mine = tmp.path().join("mine");
        for d in [&data, &root, &shared, &mine] {
            std::fs::create_dir_all(d).unwrap();
        }
        crate::zipimport::tests::make_zip(&shared.join("office.zip"), &[("a.pdf", b"a")]);
        crate::zipimport::tests::make_zip(&mine.join("mine.zip"), &[("b.pdf", b"b")]);
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![shared.clone()],
            ..Settings::default()
        };
        let d = data.clone();
        let m = mine.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 1000.0])
            .build_eframe(move |cc| {
                let mut app = FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None));
                app.set_personal_inboxes(vec![m]);
                app
            });
        settle(&mut h, "zips", |a| a.shared.intake.zips().len() == 2);
        h.get_by_label("Import").click();
        for _ in 0..6 {
            h.step();
        }
        assert!(h.query_by_label("Shared inbox (1)").is_some());
        assert!(h.query_by_label("My inbox (1)").is_some());
        assert!(h.query_all_by_label_contains("office.zip").next().is_some());
        assert!(h.query_all_by_label_contains("mine.zip").next().is_none());
        h.get_by_label("My inbox (1)").click();
        for _ in 0..6 {
            h.step();
        }
        assert!(h.query_all_by_label_contains("mine.zip").next().is_some());
        assert!(h.query_all_by_label_contains("office.zip").next().is_none());
        // No personal inboxes: no tabs, every zip.
        h.state_mut().set_personal_inboxes(vec![]);
        settle(&mut h, "shared only", |a| a.shared.intake.zips().len() == 1);
        for _ in 0..6 {
            h.step();
        }
        assert!(h.query_by_label("My inbox (0)").is_none());
        assert!(h.query_all_by_label_contains("office.zip").next().is_some());
    }

    /// The side pane shows what is inside a spreadsheet, a Word file or a mail (V27).
    #[test]
    fn side_pane_shows_inside_office_files() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
",
        )
        .unwrap();
        let book = "Smith, John_1001_2025 Trial balance.xlsx";
        let mut wb = rust_xlsxwriter::Workbook::new();
        let s = wb.add_worksheet().set_name("TB").unwrap();
        s.write(0, 0, "Petty cash").unwrap();
        s.write(0, 1, 1234.5).unwrap();
        wb.add_worksheet()
            .set_name("Adjustments")
            .unwrap()
            .write(0, 0, "AJE 1")
            .unwrap();
        wb.save(inbox.join(book)).unwrap();
        let mail = "Smith, John_1001_2025 Question.eml";
        std::fs::write(
            inbox.join(mail),
            "From: john@example.com
Subject: Mileage log

My miles for 2025.
",
        )
        .unwrap();
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 2 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 2));
        h.step();
        h.get_by_label("Preview pane").click();
        h.step();
        h.get_by_label(book).click();
        h.step();
        settle(&mut h, "workbook read", |a| a.preview.pane_contents_ready());
        h.step();
        h.step();
        assert!(h.query_by_label(&format!("Inside {book}")).is_some());
        assert!(h.query_by_label("Petty cash").is_some());
        assert!(h.query_by_label("1234.5").is_some());
        // A tab per sheet.
        h.get_by_label("Adjustments").click();
        h.step();
        h.step();
        assert!(h.query_by_label("AJE 1").is_some());
        assert!(h.query_by_label("Petty cash").is_none());

        h.get_by_label(mail).click();
        h.step();
        settle(&mut h, "mail read", |a| a.preview.pane_contents_ready());
        h.step();
        h.step();
        assert!(h.query_by_label("Mileage log").is_some());
        assert!(h.query_by_label("My miles for 2025.").is_some());
    }

    /// The side pane shows the row last clicked; ticked rows get one client and one rule at once
    /// (owner 2026-11-04).
    #[test]
    fn preview_pane_and_ticked_rows_client_and_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
1001,\"Smith, John\"
1004,Baker Farms
",
        )
        .unwrap();
        let names = [
            "Smith, John_1001_2025 1040.pdf",
            "scan0001.pdf",
            "scan0002.pdf",
        ];
        for n in names {
            crate::pdf::write_test_pdf(&inbox.join(n), &[n, "second page"]);
        }
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let pdfium = crate::pdf::find_pdfium(std::path::Path::new(""));
        assert!(pdfium.is_some());
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1800.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, pdfium))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 3 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 3));
        h.step();
        // Narrower than 1800 points: the pane opens with its switch.
        assert!(h.state().preview.pane_for_test().is_none());
        h.get_by_label("Preview pane").click();
        h.step();
        h.step();
        // The pane starts on the first row; a click on a file name shows that file.
        let first = h.state().preview.proposals()[0].file.path.clone();
        assert_eq!(h.state().preview.pane_for_test().unwrap().0, first);
        h.get_by_label("scan0002.pdf").click();
        h.step();
        h.step();
        assert_eq!(
            h.state().preview.pane_for_test().unwrap().0,
            inbox.join("scan0002.pdf")
        );
        settle(&mut h, "page drawn", |a| {
            a.preview.pane_for_test().is_some_and(|p| p.2)
        });
        // Two pages: Next shows the second.
        settle(&mut h, "page count", |_| true);
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Next page").click();
        h.step();
        settle(&mut h, "second page drawn", |a| {
            a.preview.pane_for_test().is_some_and(|p| p.1 == 1 && p.2)
        });

        // Tick the two scans: one client for both.
        tick(&mut h, "scan0001.pdf");
        tick(&mut h, "scan0002.pdf");
        h.get_by_label("Set client for ticked").click();
        settle(&mut h, "picker open", |a| a.preview.picker_open());
        h.step();
        h.get_by_label("Search by name or ID").click();
        h.step();
        h.get_by_label("Search by name or ID").type_text("1004");
        h.step();
        h.step();
        h.get_by_label("Baker Farms (1004)  ID match").click();
        settle(&mut h, "bulk client", |a| {
            ["scan0001.pdf", "scan0002.pdf"].iter().all(|n| {
                a.preview
                    .edits_for_test(&inbox.join(n))
                    .and_then(|e| e.client_id.as_deref())
                    == Some("1004")
            })
        });
        assert!(
            h.state()
                .preview
                .edits_for_test(&inbox.join(names[0]))
                .is_none_or(|e| e.client_id.is_none()),
            "the unticked row keeps its own client"
        );

        // One rule for both.
        settle(&mut h, "plan after client", |a| a.preview.plan_idle());
        let rule = h.state().shared.rules.active_rules()[1].clone();
        h.get_by_label("Apply rule to ticked").click();
        h.step();
        h.step();
        h.get_all_by_label(&rule.name).last().unwrap().click();
        settle(&mut h, "bulk rule", |a| {
            ["scan0001.pdf", "scan0002.pdf"].iter().all(|n| {
                a.preview
                    .edits_for_test(&inbox.join(n))
                    .and_then(|e| e.rule_id.as_deref())
                    == Some(rule.id.as_str())
            })
        });
        settle(&mut h, "replanned", |a| {
            a.preview.plan_idle()
                && a.preview
                    .proposals()
                    .iter()
                    .filter(|p| p.rule_id.as_deref() == Some(rule.id.as_str()))
                    .count()
                    >= 2
        });
        for _ in 0..3 {
            h.step();
        }
        crate::app::snapshot(&mut h, "preview_pane_bulk");
    }
}

#[cfg(test)]
mod split_ui_tests {
    use super::*;
    use crate::config::ColumnMap;
    use crate::plan::RowStatus;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};
    use std::path::Path;

    fn settle(h: &mut Harness<'static, FilerApp>, what: &str, done: impl Fn(&FilerApp) -> bool) {
        for _ in 0..600 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never reached: {what}");
    }

    fn plan_ready(a: &FilerApp, rows: usize) -> bool {
        a.preview.plan_idle()
            && a.preview.proposals().len() == rows
            && !a
                .preview
                .proposals()
                .iter()
                .any(|p| matches!(p.status, RowStatus::Pending(_)))
    }

    fn open_split(h: &mut Harness<'static, FilerApp>, name: &str) {
        for _ in 0..3 {
            h.step();
        }
        // Split… is in the row's right-click menu (X125).
        h.get_by_label(name).click_secondary();
        h.step();
        h.step();
        h.get_by_label("Split…").click();
        settle(h, "split dialog ready", |a| {
            a.preview.split_ready() == Some(true)
        });
        for _ in 0..3 {
            h.step();
        }
    }

    #[test]
    fn split_detect_manual_cuts_commit_and_undo() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name\n1001,\"Smith, John\"\n1002,Acme LLC\n1004,Baker Farms\n",
        )
        .unwrap();
        let run = inbox.join("Smith, John_1001_2025 print run.pdf");
        crate::pdf::write_test_pdf(
            &run,
            &[
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
            ],
        );
        let scan = inbox.join("Smith, John_1001_2025 scan.pdf");
        crate::pdf::write_test_pdf(&scan, &["", "", "", "", "", ""]);

        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let pdfium = crate::pdf::find_pdfium(Path::new(""));
        assert!(pdfium.is_some());
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 1200.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, pdfium))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 2 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview: 2 pending");
        h.get_by_label("Preview").click();
        settle(&mut h, "plan", |a| plan_ready(a, 2));

        // The print run: three pieces proposed from the client IDs in its text.
        open_split(&mut h, "Smith, John_1001_2025 print run.pdf");
        let pieces = h.state().preview.split_pieces().unwrap();
        let summary: Vec<_> = pieces
            .iter()
            .map(|(r, c)| (r.clone(), c.clone().unwrap_or_default()))
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
        settle(&mut h, "thumbnails", |a| a.preview.split_thumbs() >= 5);
        crate::app::snapshot(&mut h, "split_screen");
        h.get_by_label("Apply split · 3 rows").click();
        settle(&mut h, "pieces in the plan", |a| {
            a.preview.split_count() == 1 && plan_ready(a, 4)
        });
        h.step();
        h.get_by_label("Pages 1–4 of Smith, John_1001_2025 print run.pdf");

        // The scan: no text, one piece; cutting by hand works.
        open_split(&mut h, "Smith, John_1001_2025 scan.pdf");
        assert_eq!(
            h.state().preview.split_pieces().unwrap(),
            vec![(0..6, None)]
        );
        h.get_by_label("Cut before page 3").click();
        h.step();
        h.step();
        h.get_by_label("Cut before page 5").click();
        h.step();
        h.step();
        let ranges: Vec<_> = h
            .state()
            .preview
            .split_pieces()
            .unwrap()
            .into_iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(ranges, [0..2, 2..4, 4..6], "Done-when: manual cuts work");
        h.get_by_label("Cancel").click();
        settle(&mut h, "dialog closed", |a| {
            a.preview.split_ready().is_none()
        });

        // Commit the three pieces (the scan stays a normal row) and undo.
        settle(&mut h, "plan", |a| plan_ready(a, 4));
        h.get_by_label("Commit 4 files").click();
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        h.step();
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Confirm commit").click();
        settle(&mut h, "filed", |a| {
            a.shared.ops_generation > 0 && !a.preview.committing()
        });
        assert!(!run.exists(), "the original left the inbox");
        // Each piece's first page decides its document type (cover sheet: catch-all; 1120-S and
        // 1065 pages: Tax Return), so take the destinations from the batch record.
        let pieces: Vec<PathBuf> = h
            .state()
            .shared
            .last_batch
            .as_ref()
            .unwrap()
            .items
            .iter()
            .filter(|i| i.piece_of.is_some())
            .map(|i| i.dst.clone())
            .collect();
        assert_eq!(pieces.len(), 3);
        for (dst, folder) in pieces.iter().zip([
            "Smith, John (1001)",
            "Acme LLC (1002)",
            "Baker Farms (1004)",
        ]) {
            assert_eq!(crate::pdf::page_count(dst).unwrap(), 4, "{}", dst.display());
            assert!(dst.starts_with(root.join(folder)), "{}", dst.display());
        }
        assert!(
            pieces[1].ends_with("2025 Tax Return - Acme LLC.pdf"),
            "typed from the piece's page text"
        );
        assert_eq!(h.state().preview.split_count(), 0);

        // Badges change as the queue empties; let the header settle before clicking.
        h.step();
        h.step();
        h.get_by_label("History").click();
        settle(&mut h, "history", |a| {
            a.tab == Tab::History && !a.history.loading()
        });
        h.step();
        h.get_by_label("Undo last batch…").click();
        h.step();
        h.step();
        h.get_by_label("Confirm undo").click();
        settle(&mut h, "undone", |a| {
            a.shared.last_batch.as_ref().is_some_and(|b| b.undone)
        });
        assert!(run.exists(), "Done-when: undo restores the original");
        assert_eq!(crate::pdf::page_count(&run).unwrap(), 12);
        assert!(pieces.iter().all(|p| !p.exists()), "pieces deleted");
    }
}

#[cfg(test)]
mod p10_tests {
    use super::*;
    use crate::config::ColumnMap;
    use crate::plan::RowStatus;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn settle(h: &mut Harness<'static, FilerApp>, what: &str, done: impl Fn(&FilerApp) -> bool) {
        for _ in 0..500 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never reached: {what}");
    }

    #[test]
    fn recovery_panel_lists_and_deletes_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let leftover = data.join(crate::pdf::TMP_DIR).join("abc").join("x.pdf");
        std::fs::create_dir_all(leftover.parent().unwrap()).unwrap();
        std::fs::write(&leftover, b"%PDF temp").unwrap();
        let lb = crate::commit::LastBatch {
            batch: uuid::Uuid::new_v4(),
            committed_at: chrono::Utc::now(),
            in_progress: true,
            undone: false,
            items: vec![],
            created_dirs: vec![],
            created_files: vec![],
            backups: vec![],
            split_originals: vec![],
            zip_entries: vec![],
            zip: None,
            routed_zips: Vec::new(),
            renamed_dirs: Vec::new(),
        };
        crate::commit::save_last_batch(&data, &lb).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1300.0, 800.0])
            .build_eframe(move |cc| {
                FilerApp::new(
                    &cc.egui_ctx,
                    AppState::load(d, Settings::default(), true, None),
                )
            });
        h.step();
        h.step();
        h.get_by_label_contains("left 1 temporary PDF file in data\\tmp");
        h.get_by_label_contains("did not finish");
        crate::app::snapshot(&mut h, "recovery_panel");
        h.get_by_label("Delete leftovers…").click();
        h.step();
        h.step();
        h.get_by_label("Delete files").click();
        h.step();
        h.step();
        assert!(!leftover.exists());
        assert!(data.join(crate::pdf::TMP_DIR).exists());
        h.get_by_label("Deleted 1 temporary file.");
        // The unfinished commit is still listed (never fixed automatically).
        h.get_by_label_contains("did not finish");
    }

    #[test]
    fn keyboard_shortcuts() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1002,Acme LLC\n").unwrap();
        for n in [
            "Acme LLC_1002_2025_1040.pdf",
            "Acme LLC_1002_2025 organizer.pdf",
        ] {
            std::fs::write(inbox.join(n), b"%PDF-1.4 x").unwrap();
        }
        let settings = Settings {
            root: Some(root),
            inboxes: vec![inbox],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 800.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "queue", |a| {
            a.shared.intake.queue().len() == 2 && a.shared.clients.list().is_some()
        });
        h.get_by_label("Preview: 2 pending");
        h.get_by_label("Preview").click();
        let ready = |a: &FilerApp| {
            a.preview.plan_idle()
                && a.preview.proposals().len() == 2
                && a.preview
                    .proposals()
                    .iter()
                    .all(|p| p.status == RowStatus::Ready)
        };
        settle(&mut h, "plan", ready);
        for _ in 0..3 {
            h.step();
        }

        // Del skips the row under the mouse, and again unskips it.
        let name = "Acme LLC_1002_2025_1040.pdf";
        let status = |a: &FilerApp| {
            a.preview
                .proposals()
                .iter()
                .find(|p| p.file.path.file_name().unwrap() == name)
                .unwrap()
                .status
                .clone()
        };
        h.get_by_label(name).hover();
        h.step();
        h.key_press(egui::Key::Delete);
        settle(&mut h, "skipped", |a| {
            a.preview.plan_idle() && status(a) == RowStatus::Skipped
        });
        h.get_by_label(name).hover();
        h.step();
        h.key_press(egui::Key::Delete);
        settle(&mut h, "unskipped", ready);

        // Enter opens the Commit summary (Confirm still needs a click).
        h.key_press(egui::Key::Enter);
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Cancel").click();
        settle(&mut h, "summary closed", |a| !a.preview.summary_is_open());
        assert_eq!(h.state().shared.ops_generation, 0, "nothing was filed");

        // Ctrl+Z goes to History; with no batch yet it says why there is nothing to undo.
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        h.step();
        h.step();
        assert_eq!(h.state().tab, Tab::History);
        assert!(!h.state().history.undo_prompt_open());
        h.get_by_label_contains("Nothing to undo yet.");
    }
}

#[cfg(test)]
mod r1c_tests {
    use super::*;
    use crate::config::ColumnMap;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn settle(h: &mut Harness<'static, FilerApp>, what: &str, done: impl Fn(&FilerApp) -> bool) {
        for _ in 0..500 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never reached: {what}");
    }

    #[test]
    fn setup_cards_show_spec_pills() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name,Partner\n1001,\"Smith, John\",KW\n1002,Acme LLC,AB\n",
        )
        .unwrap();
        // A second inbox on another drive letter shows the cross-drive pill and banner.
        let other = std::path::PathBuf::from(if tmp.path().starts_with("C:") {
            r"Q:\Scans\Inbox"
        } else {
            r"C:\Scans\Inbox"
        });
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox, other],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("ID".into()),
                name: Some("Name".into()),
                extras: vec!["Partner".into()],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1200.0, 1500.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        h.state_mut().tab = Tab::Setup;
        settle(&mut h, "clients and path check", |a| {
            a.shared.clients.list().is_some() && a.setup.path_check_done()
        });
        h.step();
        h.get_by_label("writable");
        h.get_by_label("cross-drive");
        h.get_by_label("Inbox on a different volume than root.");
        h.get_by_label("valid regex");
        h.get_by_label("ID → ID");
        h.get_by_label("+ Partner as tokens");
        h.get_by_label("{Partner}");
        h.get_by_label("TOKENS AVAILABLE");
        // UltraTax card: default positions and the example; the K-1 position combo.
        h.get_by_label("John Smith_2025_1040NR_Tax_Return_John Smith*_THOMREUT.PDF");
        h.get_by_label("K1 Recipient Name");
        h.get_by_label("Restore Default");
        crate::app::snapshot(&mut h, "setup_tab");

        // Template editing: select Payroll and rename it.
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Payroll").click();
        h.step();
        h.get_by_label("Rename").click();
        h.step();
        h.step();
        h.get_by_label("Folder name").type_text(" Files");
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        assert_eq!(
            h.state().setup.template_child_names(),
            [
                "Income Tax",
                "Bookkeeping",
                "Payroll Files",
                "Financials",
                "Correspondence",
                "Workpapers & Support",
                "Sales Tax",
                "Permanent",
                "Other",
                "Not Yet Filed"
            ]
        );
        h.get_by_label("Conflicts").click();
        h.step();
        assert_eq!(h.state().tab, Tab::Conflicts);
    }
}

#[cfg(test)]
mod r2_tests {
    use super::*;
    use crate::config::ColumnMap;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::{Harness, HarnessBuilder};

    fn settle(h: &mut Harness<'static, FilerApp>, what: &str, done: impl Fn(&FilerApp) -> bool) {
        for _ in 0..500 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never reached: {what}");
    }

    /// A K-1 for a shareholder who is not related yet: "Add relationship…" opens Relationships on
    /// the entity with the name filled in; Enter adds it and returns to Preview, where the copy
    /// now resolves.
    #[test]
    fn add_relationship_from_a_copy_row() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(
            &csv,
            "id,name
4472,Riverbend Holdings LP
2093,\"Baker, Thomas\"
1042,\"Hendricks, Mark\"
",
        )
        .unwrap();
        let k1 = "Riverbend_4472_2025 K-1 Tom Baker.pdf";
        std::fs::write(inbox.join(k1), b"%PDF-1.4 k1").unwrap();
        let settings = Settings {
            root: Some(root),
            inboxes: vec![inbox],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1600.0, 900.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        let idle = |a: &FilerApp| {
            a.preview.plan_idle()
                && a.preview.proposals().len() == 1
                && !matches!(
                    a.preview.proposals()[0].status,
                    crate::plan::RowStatus::Pending(_)
                )
        };
        settle(&mut h, "plan", |a| {
            a.shared.clients.list().is_some() && idle(a)
        });
        for _ in 0..3 {
            h.step();
        }
        // "Tom Baker" only suggests (0.89 < 0.97) while Baker is not related.
        assert_eq!(
            h.state().preview.proposals()[0].status,
            crate::plan::RowStatus::NeedsClient
        );
        h.get_by_label("\"Tom Baker\" not found");
        crate::app::snapshot(&mut h, "preview_copy_unresolved");
        h.get_by_label("Add relationship…").click();
        h.step();
        h.step();
        assert_eq!(h.state().tab, Tab::Relationships);
        h.key_press(egui::Key::Enter); // focus is not in the field yet: nothing happens
        h.get_by_label("Add a child client").click();
        h.step();
        h.key_press(egui::Key::Enter);
        settle(&mut h, "related and back", |a| {
            a.shared.relationships.len() == 1 && a.tab == Tab::Preview
        });
        let r = &h.state().shared.relationships.records[0];
        assert_eq!(
            (r.parent_id.as_str(), r.child_id.as_str()),
            ("4472", "2093")
        );
        settle(&mut h, "copy resolved", |a| {
            idle(a) && a.preview.proposals()[0].status == crate::plan::RowStatus::Ready
        });
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("+1 copy");
        h.get_by_label("Commit 1 file + 1 copy");
        crate::app::snapshot(&mut h, "preview_copy_ready");
    }

    /// Without a client list the destination card still shows and says what is missing.
    #[test]
    fn import_tab_without_client_list_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        crate::zipimport::tests::make_zip(&inbox.join("docs.zip"), &[("a.pdf", b"a")]);
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1300.0, 1000.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "draft", |a| a.shared.intake.zips().len() == 1);
        h.get_by_label("Import").click();
        for _ in 0..5 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.get_by_label("pre-scan ok");
        h.get_by_label_contains("No client list yet");
    }

    /// P07b Unpack through the Import tab: a zip in the inbox becomes a draft; typing the client
    /// ID and clicking Unpack writes its files into the client folder and removes the zip.
    #[test]
    fn import_tab_unpacks_a_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1877,\"Nguyen, Anh\"\n").unwrap();
        let zip = inbox.join("Nguyen docs.zip");
        crate::zipimport::tests::make_zip(
            &zip,
            &[
                ("2024/Bank/Jan.pdf", b"jan"),
                ("2024/W2.pdf", b"w2"),
                ("run.exe", b"MZ"),
            ],
        );
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1300.0, 1000.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "draft", |a| {
            a.shared.clients.list().is_some() && a.shared.intake.zips().len() == 1
        });
        h.get_by_label("Import: 1 drafts");
        h.get_by_label("Import").click();
        for _ in 0..5 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.get_by_label("pre-scan ok");
        h.get_by_label("1 executable");
        h.get_by_label("Client").click();
        h.step();
        h.get_by_label("Client").type_text("1877");
        for _ in 0..5 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.get_by_label("1877 · exact");
        h.get_by_label_contains("None: no file in the zip");
        crate::app::snapshot(&mut h, "import_tab");
        h.get_by_label("Unpack 2 files").click();
        settle(&mut h, "unpacked", |a| a.shared.intake.zips().is_empty());
        let folder = root.join("Nguyen, Anh (1877)");
        assert_eq!(
            std::fs::read(folder.join("2024").join("Bank").join("Jan.pdf")).unwrap(),
            b"jan"
        );
        assert_eq!(
            std::fs::read(folder.join("2024").join("W2.pdf")).unwrap(),
            b"w2"
        );
        assert!(!folder.join("run.exe").exists());
        assert!(!zip.exists(), "zip removed after the import");
        assert!(
            crate::sentinel::read_sentinel(&folder).unwrap().is_some(),
            "folder created and bound"
        );
        assert!(h.state().shared.last_batch.as_ref().unwrap().zip.is_some());
    }

    /// P07b Done-when, Send to queue: the zip's PDFs land in Preview routed by the rules, a
    /// non-PDF rides along without PDF tools; after the commit the zip is gone (an undo copy is
    /// kept) and undo puts it back.
    #[test]
    fn import_tab_sends_a_zip_to_the_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let root = tmp.path().join("root");
        let inbox = tmp.path().join("inbox");
        for d in [&data, &root, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let csv = tmp.path().join("clients.csv");
        std::fs::write(&csv, "id,name\n1877,\"Nguyen, Anh\"\n").unwrap();
        let zip = inbox.join("Nguyen docs.zip");
        crate::zipimport::tests::make_zip(
            &zip,
            &[
                ("2025/Nguyen_1877_2025 1040.pdf", b"%PDF-1.4 return"),
                ("Nguyen_1877_2025 notes.txt", b"notes"),
                ("Thumbs.db", b"x"),
                ("run.exe", b"MZ"),
            ],
        );
        let zip_bytes = std::fs::read(&zip).unwrap();
        let settings = Settings {
            root: Some(root.clone()),
            inboxes: vec![inbox.clone()],
            csv_path: Some(csv),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1300.0, 1000.0])
            .build_eframe(move |cc| {
                crate::rules::seed_tax(&d);
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "draft", |a| {
            a.shared.clients.list().is_some() && a.shared.intake.zips().len() == 1
        });
        h.get_by_label("Import: 1 drafts").click();
        for _ in 0..5 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        h.get_by_label("pre-scan ok");
        h.get_by_label("Send contents to the queue").click();
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Queue 2 files").click();
        settle(&mut h, "queued", |a| a.shared.intake.queue().len() == 2);
        assert!(
            h.state().shared.intake.zips().is_empty(),
            "no longer a draft"
        );
        assert!(zip.exists(), "the zip stays until its files are filed");
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Preview: 2 pending").click();
        settle(&mut h, "plan", |a| {
            a.preview.plan_idle()
                && a.preview.proposals().len() == 2
                && !a
                    .preview
                    .proposals()
                    .iter()
                    .any(|p| matches!(p.status, crate::plan::RowStatus::Pending(_)))
        });
        let props = h.state().preview.proposals().to_vec();
        let ret = props
            .iter()
            .find(|p| p.file.path.extension().is_some_and(|e| e == "pdf"))
            .unwrap();
        assert_eq!(ret.rule_name, "Tax Return", "routed by the rules");
        assert_eq!(ret.status, crate::plan::RowStatus::Ready);
        assert!(matches!(
            &ret.file.source,
            crate::intake::Source::Zip { entry, .. } if entry == "2025/Nguyen_1877_2025 1040.pdf"
        ));
        h.step();
        h.get_by_label("no PDF tools");
        crate::app::snapshot(&mut h, "preview_zip");

        h.get_by_label("Commit 2 files").click();
        settle(&mut h, "summary", |a| a.preview.summary_is_open());
        for _ in 0..4 {
            h.step();
        }
        h.get_by_label_contains("2 files from Nguyen docs.zip");
        h.get_by_label("Confirm commit").click();
        settle(&mut h, "filed and zip finished", |a| {
            !a.preview.committing() && a.shared.routed.is_empty() && a.shared.last_batch.is_some()
        });
        let returns = root
            .join("Nguyen, Anh (1877)")
            .join("Tax Returns")
            .join("2025");
        let filed: Vec<_> = std::fs::read_dir(&returns).unwrap().flatten().collect();
        assert_eq!(filed.len(), 1, "{filed:?}");
        assert_eq!(std::fs::read(filed[0].path()).unwrap(), b"%PDF-1.4 return");
        assert!(!zip.exists(), "zip removed once all its files were filed");
        let lb = h.state().shared.last_batch.clone().unwrap();
        assert_eq!(lb.routed_zips.len(), 1, "undo copy of the zip");
        let kept = lb.routed_zips[0].kept.clone();
        assert_eq!(std::fs::read(&kept).unwrap(), zip_bytes);
        let ops = crate::oplog::read(&crate::oplog::path(&data))
            .unwrap()
            .records;
        let imp = ops
            .iter()
            .find(|r| r.op == crate::oplog::Op::ZipImport)
            .unwrap();
        assert!(
            imp.note
                .as_deref()
                .unwrap()
                .starts_with("queue · 2 queued · 2 filed"),
            "{imp:?}"
        );
        let mv = ops
            .iter()
            .find(|r| {
                r.dst
                    .as_ref()
                    .is_some_and(|d| d.parent() == Some(returns.as_path()))
            })
            .unwrap();
        assert_eq!(
            mv.note.as_deref(),
            Some("Nguyen docs.zip › 2025/Nguyen_1877_2025 1040.pdf")
        );
        assert!(
            crate::recovery::Recovery::scan(&data, None)
                .leftovers
                .is_empty(),
            "undo zip is not a leftover"
        );

        let undo = crate::undo::run_undo(&data, lb, false);
        assert!(undo.warnings.is_empty(), "{:?}", undo.warnings);
        assert_eq!(std::fs::read(&zip).unwrap(), zip_bytes, "the zip is back");
        assert!(!returns.join("2025 Tax Return - Nguyen, Anh.pdf").exists());
    }

    /// Two zips sent to the queue and finished by one batch (one entry filed as split pieces, the
    /// other directly): both are deleted with an undo copy each, and undo puts both back.
    #[test]
    fn routed_zips_finished_by_one_batch_split_or_direct() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let inbox = tmp.path().join("inbox");
        let root = tmp.path().join("root");
        for d in [&data, &inbox, &root] {
            std::fs::create_dir_all(d).unwrap();
        }
        let mut state = AppState::load(data.clone(), Settings::default(), true, None);
        let mut routed = Vec::new();
        for (k, name) in ["A.zip", "B.zip"].iter().enumerate() {
            let zip = inbox.join(name);
            std::fs::write(&zip, format!("zip {k}")).unwrap();
            let entry = data.join("tmp").join(format!("zip-{k}")).join("doc.pdf");
            routed.push(crate::app::RoutedZip {
                id: uuid::Uuid::new_v4(),
                zip,
                listed: None,
                tmp: entry.parent().unwrap().to_path_buf(),
                entries: vec![entry],
                files: Vec::new(),
                filed: Vec::new(),
            });
        }
        let item = |src: PathBuf, piece_of: Option<PathBuf>| commit::FiledItem {
            src,
            dst: root.join("x.pdf"),
            method: commit::Method::Move,
            bytes: 1,
            client_id: "1".into(),
            return_to: None,
            merged: Vec::new(),
            combined: false,
            piece_of,
            copies: Vec::new(),
            from_zip: None,
        };
        let lb = commit::LastBatch {
            batch: uuid::Uuid::new_v4(),
            committed_at: chrono::Utc::now(),
            in_progress: false,
            undone: false,
            items: vec![
                item(
                    data.join("tmp").join("split").join("1").join("doc.pdf"),
                    Some(routed[0].entries[0].clone()),
                ),
                item(routed[1].entries[0].clone(), None),
            ],
            created_dirs: Vec::new(),
            created_files: Vec::new(),
            backups: Vec::new(),
            split_originals: Vec::new(),
            zip_entries: Vec::new(),
            zip: None,
            routed_zips: Vec::new(),
            renamed_dirs: Vec::new(),
        };
        state.last_batch = Some(lb);
        state.routed = routed;
        state.finish_routed_zips();
        assert!(state.routed.is_empty());
        let lb = state.last_batch.clone().unwrap();
        assert_eq!(lb.routed_zips.len(), 2, "both zips kept for undo");
        assert!(!inbox.join("A.zip").exists() && !inbox.join("B.zip").exists());
        let undo = crate::undo::run_undo(&data, lb, false);
        assert_eq!(std::fs::read(inbox.join("A.zip")).unwrap(), b"zip 0");
        assert_eq!(std::fs::read(inbox.join("B.zip")).unwrap(), b"zip 1");
        let _ = undo;
    }

    /// P07a Done-when, last clause: add, re-link and remove without touching the CSV.
    #[test]
    fn relationships_tab_adds_relinks_and_removes() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let csv = tmp.path().join("clients.csv");
        let csv_text = "id,name\n1234,Riverbend Holdings LP\n2093,\"Baker, Thomas\"\n1042,\"Hendricks, Mark\"\n";
        std::fs::write(&csv, csv_text).unwrap();
        // A dangling relationship from an earlier client list.
        std::fs::write(
            crate::relationships::path(&data),
            r#"[{"parent_id":"1234","child_id":"5120","role":"partner","note":"Morales"}]"#,
        )
        .unwrap();
        let settings = Settings {
            csv_path: Some(csv.clone()),
            column_map: ColumnMap {
                id: Some("id".into()),
                name: Some("name".into()),
                extras: vec![],
            },
            ..Settings::default()
        };
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1200.0, 900.0])
            .build_eframe(move |cc| {
                FilerApp::new(&cc.egui_ctx, AppState::load(d, settings, true, None))
            });
        settle(&mut h, "clients", |a| a.shared.clients.list().is_some());
        h.get_by_label("Relationships").click();
        h.step();
        h.step();
        h.get_by_label("Riverbend Holdings LP").click();
        h.step();
        h.step();
        h.get_by_label("dangling");

        // Add Baker as a child (type the ID, press Enter).
        h.get_by_label("Add a child client").click();
        h.step();
        h.get_by_label("Add a child client").type_text("2093");
        h.step();
        h.key_press(egui::Key::Enter);
        settle(&mut h, "added", |a| a.shared.relationships.len() == 2);
        let saved = std::fs::read_to_string(crate::relationships::path(&data)).unwrap();
        assert!(saved.contains("\"2093\""), "saved on every edit: {saved}");
        crate::app::snapshot(&mut h, "relationships_tab");

        // Re-link the dangling 5120 to Hendricks (1042).
        h.get_by_label("Re-link…").click();
        h.step();
        h.step();
        h.get_by_label("Search by name or ID").click();
        h.step();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label("Search by name or ID").type_text("1042");
        h.step();
        h.step();
        h.get_by_label_contains("Hendricks, Mark (1042)").click();
        settle(&mut h, "relinked", |a| {
            a.shared
                .relationships
                .records
                .iter()
                .any(|r| r.child_id == "1042")
        });

        // Remove Baker again.
        for _ in 0..3 {
            h.step();
        }
        h.get_by_label("Remove relationship with 2093").click();
        settle(&mut h, "removed", |a| a.shared.relationships.len() == 1);
        let saved = std::fs::read_to_string(crate::relationships::path(&data)).unwrap();
        assert!(
            saved.contains("\"1042\"") && !saved.contains("\"2093\"") && !saved.contains("5120")
        );
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap(),
            csv_text,
            "CSV untouched"
        );
    }
}
