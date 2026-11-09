//! Intake tab (P02 step 1): the vendored Filer Desktop app (`filer_intake::app::FilerApp`), mounted
//! unchanged.
//!
//! Filer keeps all of its files in one data folder. Vibe Files points it at `<cabinet>/_Vibe/`
//! (QUESTIONS Q-19): `clients.json`, `relationships.json`, `template.json` there are the cabinet's
//! client master already, so Filer and Browse share one master. Filer's other files
//! (`config.json`, `rules.json`, `folders.json`, `last_batch.json`, `ops.jsonl`, `tmp/`,
//! `overwritten/`, `undo_zips/`, `split_originals/`) are cabinet-level too, which is safe only
//! with one writer at a time: two PCs must not run Intake on one cabinet together until the P09
//! catalog-writer lock gates it.

use std::path::{Path, PathBuf};

use filer_intake::app::{AppState, FilerApp};
use filer_intake::config::{Settings, ThemeChoice};
use vf_core::db::Connection;
use vf_fs::cabinet::CabinetFile;

use crate::config::Ingest;
use crate::ui::ingest::IngestState;
use crate::ui::theme;
use vf_core::browse::Snapshot;

#[derive(Default)]
pub struct IntakeTab {
    /// How many times Filer's screen was loaded (tests: once, not every frame).
    #[cfg(test)]
    pub mounts: usize,
    /// Built the first time the tab is shown (its `new` starts the inbox watcher and the
    /// first-page sniffer threads).
    filer: Option<FilerApp>,
    /// This PC's Intake folder, while Filer is loaded from it (Q-53).
    pc: Option<PathBuf>,
    /// This PC's copies of the shared Filer files, kept in step with the cabinet's.
    shared: vf_fs::intake::Shared,
    error: Option<String>,
    /// `last_batch.json`'s modification time when the catalog was last brought in line with it.
    seen: Option<std::time::SystemTime>,
    /// After a failed reconcile: not tried again before this, and the wait so far (doubling to a
    /// minute), so a share that is gone does not stall every frame (QA A5).
    retry_at: Option<std::time::Instant>,
    backoff: std::time::Duration,
    /// A reconcile running on its thread: the `last_batch.json` time it started from.
    reconciling: Option<(Option<std::time::SystemTime>, ReconcileRx)>,
    /// Filer's rules.json was rewritten from Admin (print capture folders) while Filer had
    /// its rules loaded: Intake offers to reload.
    rules_stale: bool,
    /// The theme Filer's screen was loaded with (`None` = follow Windows): when Vibe's own
    /// button changes it, Filer's theme button would show the old one, so the screen is loaded
    /// again once nothing waits in its review (review 2026-11-04).
    filer_look: Option<Option<bool>>,
    /// The documents Intake's last batch filed, being given their details (owner 2026-11-04).
    just_filed: Option<crate::ui::filed_details::JustFiled>,
    /// A batch was recorded: list it on the next frame.
    want_details: bool,
    /// Filer's client list changed after Intake loaded it (a new client): reload offered.
    clients_stale: bool,
    /// "New client…" pressed on a row Filer found no client for: Vibe's New client dialog, for
    /// that file (owner 2026-11-04).
    new_client: Option<(crate::ui::new_client::NewClientDialog, PathBuf)>,
    /// `clients.json`'s modified time when Filer loaded it.
    clients_at: Option<std::time::SystemTime>,
    /// rules.json's modified time when last brought in line, and when it was last looked at:
    /// a save from Filer's own Rules screen (with the rules it loaded) is noticed and the
    /// print capture rules put back (QA2 U5).
    rules_seen: Option<std::time::SystemTime>,
    rules_looked: Option<std::time::Instant>,
    /// rules.json's modified time when Filer was loaded (the rules it holds).
    filer_rules_at: Option<std::time::SystemTime>,
    /// Whether Filer is filing a batch, and when that was last read (once a second).
    committing: (bool, Option<std::time::Instant>),
    /// The catalog changed (a batch was recorded or undone): Browse re-reads it.
    pub catalog_changed: bool,
    /// What the last reconcile did or why it failed, for Browse's notice.
    pub message: Option<String>,
    /// Vibe's staging folders and barcode sheets, shown above Filer.
    pub ingest: IngestState,
    /// Documents Filer just filed whose content was already in the cabinet (Q-27).
    pub dupes: Vec<vf_fs::dupes::Dup>,
    /// A choice made on one of `dupes`, applied on the next reconcile (it needs the catalog).
    dupe_action: Option<(usize, vf_fs::dupes::Choice)>,
    /// Inbox files no rule names that were handed to the form reading (Q-61), once each.
    read_asked: std::collections::HashSet<PathBuf>,
    /// What the form reading found, from its thread.
    reads: Option<Reads>,
    /// Tells the reading threads to stop (the cabinet closed).
    read_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for IntakeTab {
    fn drop(&mut self) {
        self.read_stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The form reading's channel: its threads send, the tab takes.
type Reads = (
    std::sync::mpsc::Sender<(PathBuf, Option<vf_ingest::recognize::InboxReading>)>,
    std::sync::mpsc::Receiver<(PathBuf, Option<vf_ingest::recognize::InboxReading>)>,
);

/// The window's theme as Filer's settings name it: `None` = follow Windows.
fn current_look(ctx: &egui::Context) -> Option<bool> {
    if ctx.options(|o| o.theme_preference) == egui::ThemePreference::System {
        None
    } else {
        Some(ctx.theme() == egui::Theme::Dark)
    }
}

/// Filer's ID pattern for client IDs of letters and/or digits (`STOC1234`, `1042`).
pub const ID_PATTERN: &str = r"[A-Z0-9]{4,}";

/// Filer's settings for this cabinet on this PC: whatever `_Vibe/config.json` holds (rules
/// profile, inboxes, column map, …), with the cabinet's own facts written over it, and Vibe's
/// theme and text size so Filer's screen does not switch them when it loads (owner 2026-11-04);
/// those two are never saved into the shared `config.json`. `dark`: `None` = follow Windows.
pub fn settings_with_theme(
    root: &Path,
    cabinet: &CabinetFile,
    dark: Option<bool>,
    zoom: f32,
) -> Result<(Settings, bool), String> {
    let dir = vf_fs::clients::master_dir(root);
    let (mut s, savable) = Settings::load_or_default(&dir);
    let before = serde_json::to_value(&s).ok();
    // The root as this PC opened it (a mapped drive here can be a UNC path elsewhere).
    s.root = Some(root.to_path_buf());
    // Every sentinel Filer writes must carry the cabinet's firm id.
    s.firm_id = Some(
        uuid::Uuid::parse_str(&cabinet.firm_id)
            .map_err(|e| format!("cabinet.json firm_id is not a uuid: {e}"))?,
    );
    // The theme and text size are set after the save below: they are this PC's (config.toml),
    // not the cabinet's shared config.json.
    // The office's UltraTax client IDs are letters and digits; Filer's default ID pattern is
    // digits only. A pattern the firm chose is kept (owner 2026-11-03).
    if s.id_pattern == filer_core::parse::DEFAULT_ID_PATTERN {
        s.id_pattern = ID_PATTERN.into();
    }
    // UltraTax names are read once, by Vibe's print capture (Admin › Print capture & parsers),
    // which renames each print for Filer: Filer's own UltraTax reading (0.2.5+) stays off so
    // there is one place to set it up (owner 2026-11-04, no duplicate processing).
    s.ultratax.enabled = false;
    // Filer re-imports the client CSV on launch when it changed; point it at the one the master
    // list came from.
    if let Ok(Some(list)) = filer_core::clients::load_cache(&dir)
        && !list.source.as_os_str().is_empty()
        && list.source.is_file()
    {
        s.csv_path = Some(list.source.clone());
        s.column_map = list.column_map.clone();
    }
    if savable && serde_json::to_value(&s).ok() != before {
        s.save(&dir)
            .map_err(|e| format!("saving Filer's config.json: {e}"))?;
    }
    // Vibe's theme and text size for Filer's screen, not Windows' or Filer's own.
    s.display.theme = match dark {
        None => ThemeChoice::System,
        Some(true) => ThemeChoice::Dark,
        Some(false) => ThemeChoice::Light,
    };
    s.display.zoom = filer_core::config::Display::clamp_zoom(zoom);
    Ok((s, savable))
}

/// The folder holding a `pdfium.dll` that binds (next to the exe; debug builds also find the
/// vendored copy), or `None` (Filer's PDF features off).
pub(crate) fn pdfium_dir() -> Option<PathBuf> {
    let exe_dir = vf_core::exe_dir()?;
    filer_pdf::pdf::find_pdfium(&exe_dir)
}

/// Whether Filer is filing a batch now (its `last_batch.json` says in progress).
/// A flag left by a crash mid-commit (Filer never clears it by itself) is not believed once the
/// file has not changed for 15 minutes (a commit writes it when it starts and ends; one
/// running longer than that is rare and only re-enables the button) (QA3 #3).
fn filer_committing(dir: &Path) -> bool {
    let p = filer_intake::commit::last_batch_path(dir);
    let recent = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(15 * 60));
    recent
        && std::fs::read(p)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("in_progress").and_then(|x| x.as_bool()))
            .unwrap_or(false)
}

/// What a reconcile found: the batch recorded or undone, and the duplicates it filed.
type Reconcile = (
    vf_fs::intake::Reconciled,
    Result<Vec<vf_fs::dupes::Dup>, String>,
);

type ReconcileRx = std::sync::mpsc::Receiver<Result<Reconcile, String>>;

/// [`vf_fs::intake::reconcile`] and the duplicate check of what it filed.
fn reconcile_now(conn: &mut Connection, root: &Path, device_id: &str) -> Result<Reconcile, String> {
    let before = vf_core::now_rfc3339();
    let r = vf_fs::intake::reconcile(conn, root, device_id)?;
    let dupes = if r.recorded.is_some() {
        vf_fs::dupes::filed_since(conn, &before)
    } else {
        Ok(vec![])
    };
    Ok((r, dupes))
}

impl IntakeTab {
    #[cfg(test)]
    pub fn debug_state(&self) -> String {
        format!(
            "mounts={} look={:?} filer={} error={:?}",
            self.mounts,
            self.filer_look,
            self.filer.is_some(),
            self.error
        )
    }

    /// The folder this PC's Filer works in (its own once loaded; the cabinet's before).
    fn filer_dir(&self, root: &Path) -> PathBuf {
        self.pc
            .clone()
            .unwrap_or_else(|| vf_fs::clients::master_dir(root))
    }

    /// Records Filer's last batch in the catalog when `last_batch.json` changed since the last
    /// look (a stat per frame otherwise). See `vf_fs::intake`. The work (a walk of the client
    /// folders and a hash of each filed file) runs on its own thread with its own catalog
    /// connection, so a big batch or a slow share never freezes the window (QA A5); its result
    /// is taken on a later frame. A failure is retried after 2 s, doubling to a minute.
    pub fn reconcile(
        &mut self,
        conn: &mut Connection,
        catalog: &Path,
        root: &Path,
        device_id: &str,
    ) {
        self.apply_dupe_action(conn, root, device_id);
        if self.poll_reconcile() {
            return;
        }
        // Every PC's last batch (Q-53): any of them changing is looked at.
        let modified = vf_fs::intake::latest_change(root);
        if modified.is_none() || modified == self.seen {
            return;
        }
        if self.retry_at.is_some_and(|t| std::time::Instant::now() < t) {
            return;
        }
        // Tests take the result on the same frame.
        if cfg!(test) {
            let r = reconcile_now(conn, root, device_id);
            self.reconciled(modified, r);
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (catalog, root, device) = (
            catalog.to_path_buf(),
            root.to_path_buf(),
            device_id.to_string(),
        );
        let started = std::thread::Builder::new()
            .name("intake-reconcile".into())
            .spawn(move || {
                let r = vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                    .map_err(|e| e.to_string())
                    .and_then(|mut c| reconcile_now(&mut c, &root, &device));
                let _ = tx.send(r);
            });
        match started {
            Ok(_) => self.reconciling = Some((modified, rx)),
            Err(e) => self.reconciled(modified, Err(e.to_string())),
        }
    }

    /// Every 2 s on the Intake tab: when rules.json changed since it was last brought in line
    /// (Filer's Rules screen saved), the print capture rules are put back, and Intake is offered
    /// a reload if that changed anything.
    fn watch_rules(&mut self, root: &Path) {
        if self
            .rules_looked
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2))
        {
            return;
        }
        self.rules_looked = Some(std::time::Instant::now());
        if let (Some(pc), true) = (self.pc.clone(), self.filer.is_some()) {
            match self.shared.poll(root, &pc) {
                Ok(ch) => {
                    let master = vf_fs::clients::master_dir(root);
                    let at = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                    // This PC's own saves: the cabinet now has what Filer has loaded.
                    if ch.to_cabinet.contains(&"rules.json") {
                        self.filer_rules_at = at(filer_core::rules::path(&master));
                    }
                    if ch.to_cabinet.contains(&"clients.json") {
                        self.clients_at = at(filer_core::clients::cache_path(&master));
                    }
                    // Changed elsewhere: rules and clients are noticed below; the others offer
                    // the reload here.
                    if ch
                        .from_cabinet
                        .iter()
                        .any(|f| !matches!(*f, "rules.json" | "clients.json"))
                    {
                        self.clients_stale = true;
                    }
                }
                Err(e) => tracing::warn!(
                    kind = crate::logging::kind(&e),
                    "shared Intake files not in step"
                ),
            }
        }
        let path = filer_core::rules::path(&vf_fs::clients::master_dir(root));
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let now = modified(&path);
        if now.is_none() || now == self.rules_seen {
            self.look_at_clients(root);
            return;
        }
        // Brought in line (also on the first look: a save made elsewhere before it); recorded
        // only when that worked, so a failure is tried again (QA3 #11).
        if vf_ingest::parsers::sync_filer_rules_report(root).is_err() {
            return;
        }
        self.rules_seen = modified(&path);
        self.look_at_clients(root);
        // Filer loaded other rules than the file has now (written here, by Filer's own Rules
        // screen, or by another PC): offer the reload (QA3 #4).
        if self.filer.is_some() && self.filer_rules_at != self.rules_seen {
            self.rules_stale = true;
        }
    }

    /// A client added since Filer loaded its list (Browse › New client, a CSV import, another
    /// PC): Filer's client box would not offer it until Intake reloads (owner 2026-11-04).
    fn look_at_clients(&mut self, root: &Path) {
        if self.filer.is_none() {
            return;
        }
        let p = filer_core::clients::cache_path(&vf_fs::clients::master_dir(root));
        let now = std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if now.is_some() && now != self.clients_at {
            self.clients_stale = true;
        }
    }

    /// The New client dialog opened from an Intake row: the client is created as from Browse
    /// (folder from the template, sentinel, client list, catalog), and the row gets it.
    fn new_client_dialog(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        conn: &mut vf_core::db::Connection,
        device_id: &str,
    ) {
        use crate::ui::widgets::DialogOutcome;
        let Some((d, file)) = &mut self.new_client else {
            return;
        };
        let mut env = crate::ui::browse::Env {
            conn,
            root,
            device_id,
        };
        match crate::ui::new_client::show(ctx, d, &mut env) {
            DialogOutcome::Open => {}
            DialogOutcome::Cancelled => self.new_client = None,
            DialogOutcome::Done(created) => {
                let file = file.clone();
                self.new_client = None;
                let ext: Option<String> = conn
                    .query_row(
                        "SELECT ext_id FROM client WHERE id = ?1",
                        [&created.client_id],
                        |r| r.get(0),
                    )
                    .ok();
                // This PC's Intake folder gets the new client list now (not at the next poll),
                // and it is the list Filer is about to load: no "Reload Intake" for it.
                if let Some(pc) = self.pc.clone() {
                    let _ = self.shared.poll(root, &pc);
                }
                self.clients_at = std::fs::metadata(filer_core::clients::cache_path(
                    &vf_fs::clients::master_dir(root),
                ))
                .and_then(|m| m.modified())
                .ok();
                if let (Some(f), Some(ext)) = (&mut self.filer, ext) {
                    f.client_created(ctx, &file, &ext);
                }
                self.catalog_changed = true;
                self.message = Some(format!("Created {}.", created.folder));
            }
        }
    }

    /// Admin rewrote Filer's rules: a loaded Filer must reload them (offered on the tab).
    pub fn rules_changed(&mut self) {
        self.rules_stale = self.filer.is_some();
    }

    /// Takes a finished reconcile's result (every frame, whatever tab shows). Returns whether
    /// one is still running.
    pub fn poll_reconcile(&mut self) -> bool {
        let Some((_, rx)) = &self.reconciling else {
            return false;
        };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the work stopped unexpectedly".into())
            }
        };
        let (modified, _) = self.reconciling.take().expect("checked");
        self.reconciled(modified, r);
        false
    }

    fn reconciled(
        &mut self,
        modified: Option<std::time::SystemTime>,
        r: Result<Reconcile, String>,
    ) {
        match r {
            Ok((r, dupes)) => {
                self.seen = modified;
                self.retry_at = None;
                self.backoff = std::time::Duration::ZERO;
                if let Some((_, added, updated)) = r.recorded {
                    self.want_details = added > 0;
                    self.message = Some(format!(
                        "Intake filed {added} new documents{}.",
                        if updated > 0 {
                            format!(" and replaced {updated}")
                        } else {
                            String::new()
                        }
                    ));
                    match dupes {
                        Ok(d) => self.dupes.extend(d),
                        Err(e) => self.message = Some(format!("Duplicate check failed: {e}")),
                    }
                } else if r.undone.is_some() {
                    self.message = Some("Intake's last batch was undone.".into());
                }
                self.catalog_changed |= r.changed();
            }
            // Tried again later (e.g. a file still being written); keep the message.
            Err(e) => {
                self.backoff = (self.backoff * 2).clamp(
                    std::time::Duration::from_secs(2),
                    std::time::Duration::from_secs(60),
                );
                self.retry_at = Some(std::time::Instant::now() + self.backoff);
                self.message = Some(format!(
                    "Intake batch not recorded yet (trying again in {} s): {e}",
                    self.backoff.as_secs()
                ));
            }
        }
    }

    fn apply_dupe_action(&mut self, conn: &mut Connection, root: &Path, device_id: &str) {
        let Some((i, choice)) = self.dupe_action.take() else {
            return;
        };
        if i >= self.dupes.len() {
            return;
        }
        let d = self.dupes.remove(i);
        match vf_fs::dupes::resolve(conn, root, device_id, &d, choice) {
            Ok(_) => {
                self.catalog_changed = true;
                self.message = Some(match choice {
                    vf_fs::dupes::Choice::RemoveNew => format!("Moved {} to the trash.", d.new_rel),
                    vf_fs::dupes::Choice::ReplaceOld => {
                        format!("Moved {} to the trash.", d.old_rel)
                    }
                    vf_fs::dupes::Choice::KeepBoth => format!("Kept both copies of {}.", d.new_rel),
                });
            }
            Err(e) => {
                self.message = Some(format!("Could not resolve the duplicate: {e}"));
                self.dupes.insert(i, d);
            }
        }
    }

    /// Filed duplicates, one line each with the three choices.
    fn dupes_panel(&mut self, ui: &mut egui::Ui) {
        if self.dupes.is_empty() {
            return;
        }
        egui::Panel::top("vf-dupes")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(crate::ui::theme::soft(egui::Color32::from_rgb(0xFF, 0xF6, 0xE0)))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Already in the cabinet").strong());
                for (i, d) in self.dupes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} has the same content as {}.", d.new_rel, d.old_rel));
                        use vf_fs::dupes::Choice;
                        for (label, c) in [
                            ("Remove new copy", Choice::RemoveNew),
                            ("Keep both", Choice::KeepBoth),
                            ("Replace old", Choice::ReplaceOld),
                        ] {
                            if ui.button(label).clicked() {
                                self.dupe_action = Some((i, c));
                            }
                        }
                    });
                }
                ui.label(crate::ui::widgets::caption(
                    "Removed copies go to the cabinet trash. Removing a file from Filer's last batch makes that batch's Undo unavailable.",
                ));
            });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        root: &Path,
        cabinet: &CabinetFile,
        prefs: &Ingest,
        snap: &Snapshot,
        sheets_dir: &Path,
        conn: &mut vf_core::db::Connection,
        device_id: &str,
        setup_locked: bool,
    ) {
        self.ingest.strip(ui, prefs, snap, sheets_dir, root);
        if std::mem::take(&mut self.want_details) {
            self.just_filed = crate::ui::filed_details::JustFiled::load(conn);
        }
        if let Some(jf) = &mut self.just_filed {
            use crate::ui::filed_details::Act;
            match crate::ui::filed_details::show(ui, jf, &snap.pickable_doc_types(None)) {
                Act::None => {}
                Act::Close => self.just_filed = None,
                Act::Apply => match jf.apply(conn, root, device_id) {
                    Ok(n) => {
                        jf.error = None;
                        self.catalog_changed = true;
                        self.message = Some(format!("Details set on {n} documents."));
                    }
                    Err(e) => jf.error = Some(e),
                },
            }
        }
        self.watch_rules(root);
        if self.rules_stale || self.clients_stale {
            let mut reload = false;
            // Not while Filer is filing a batch: its commit thread outlives the screen (QA2 U4).
            if self
                .committing
                .1
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(1))
            {
                self.committing = (
                    filer_committing(&self.filer_dir(root)),
                    Some(std::time::Instant::now()),
                );
            }
            let committing = self.committing.0;
            egui::Panel::top("vf-intake-rules-stale").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(if self.rules_stale {
                        "Where print captures are filed changed (Admin › Print capture). Reload Intake to use it; changes not yet confirmed in Intake's review are read again from the inbox."
                    } else {
                        "The client list or Intake's settings changed (another PC, or Admin). Reload Intake to use them; changes not yet confirmed in Intake's review are read again from the inbox."
                    });
                    reload = ui
                        .add_enabled(!committing, egui::Button::new("Reload Intake"))
                        .on_disabled_hover_text("Intake is filing a batch: wait for it to finish.")
                        .clicked();
                });
            });
            if reload {
                self.filer = None;
                self.rules_stale = false;
                self.clients_stale = false;
            }
        }
        self.dupes_panel(ui);
        if let Some(seen) = self.filer_look
            && self.filer.is_some()
            && seen != current_look(ui.ctx())
            && self.ingest.badge() == 0
            && !filer_committing(&self.filer_dir(root))
        {
            self.filer = None;
        }
        if self.filer.is_none() && self.error.is_none() {
            let look = current_look(ui.ctx());
            match settings_with_theme(root, cabinet, look, ui.ctx().zoom_factor()) {
                Ok((settings, savable)) => {
                    // This PC's own Intake folder (Q-53: several people file at once), with
                    // the cabinet's shared files copied in.
                    let pc = vf_fs::intake::pc_dir(root, device_id);
                    if let Err(e) = self.shared.prepare(root, &pc) {
                        self.error = Some(format!("Intake's folder for this PC: {e}"));
                        return;
                    }
                    let state = AppState::load(pc.clone(), settings, savable, pdfium_dir());
                    self.pc = Some(pc);
                    let ctx = ui.ctx().clone();
                    self.filer = Some(FilerApp::new(&ctx, state));
                    self.filer_rules_at = std::fs::metadata(filer_core::rules::path(
                        &vf_fs::clients::master_dir(root),
                    ))
                    .and_then(|m| m.modified())
                    .ok();
                    self.rules_seen = None;
                    // The look Filer was given, not read back now: Filer's `new` sets
                    // "follow Windows" until its first frame applies its settings, and reading
                    // that here reloaded Filer on every frame (found 2026-11-04).
                    self.filer_look = Some(look);
                    if look.is_none() {
                        ui.ctx().set_theme(egui::ThemePreference::System);
                    }
                    #[cfg(test)]
                    {
                        self.mounts += 1;
                    }
                    self.clients_at = std::fs::metadata(filer_core::clients::cache_path(
                        &vf_fs::clients::master_dir(root),
                    ))
                    .and_then(|m| m.modified())
                    .ok();
                    self.clients_stale = false;
                    // Filer's `new` installs its theme; put Vibe's tweaks back on top.
                    theme::install(&ctx);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(f) = &mut self.filer {
            // Intake › Setup changes the cabinet's shared settings and folder template: behind
            // the admin password like Admin's pages (owner 2026-11-04).
            f.set_setup_lock(setup_locked.then(|| {
                "Intake's Setup is locked by the admin password. Unlock it on the Admin tab to change these settings."
                    .to_string()
            }));
            f.offer_new_client(true);
            if let Some(req) = f.take_new_client_request() {
                let mut d = crate::ui::new_client::NewClientDialog::new(req.id.clone());
                d.req.name = req.name.trim().to_string();
                self.new_client = Some((d, req.file));
            }
            // This Windows login's own inboxes (Q-64), besides the shared ones.
            let login = vf_core::windows_login().unwrap_or_default();
            let mine = prefs.my_inboxes(&login).to_vec();
            f.set_personal_inboxes(mine.clone());
            // A double-clicked file name opens in its Windows program through Browse's shell
            // (testable). Not the Viewer: it edits catalogued documents, and an inbox file is
            // not one yet (owner 2026-11-09, V24).
            f.offer_open(true);
            // Move to trash: the files go to the cabinet trash, restorable to the inbox (Q-61).
            f.offer_trash(true);
            if let Some(files) = f.take_trash_request() {
                // Only from the inboxes Intake is set to (the check inside refuses others).
                let dirs = vf_fs::trash::inboxes(root);
                match vf_fs::trash::delete_inbox_files_with(
                    conn, root, device_id, &files, &dirs, &mine,
                ) {
                    Ok(_) => {
                        f.files_taken(&files);
                        self.catalog_changed = true;
                        self.message = Some(match files.as_slice() {
                            [one] => format!(
                                "Moved {} to the trash; Restore there puts it back in the inbox.",
                                one.file_name()
                                    .map(|n| n.to_string_lossy())
                                    .unwrap_or_default()
                            ),
                            many => format!(
                                "Moved {} files to the trash; Restore there puts them back in the inbox.",
                                many.len()
                            ),
                        });
                    }
                    Err(e) => self.message = Some(format!("Not moved to the trash: {e}")),
                }
            }
            // Form reading for files whose name no rule names (Q-61): on a thread, once each.
            let unnamed = f.unnamed_files();
            // A file that left the list (filed, trashed) is forgotten: a later file of the same
            // name (scanners reuse names) is read again (QA 2026-11-05).
            self.read_asked.retain(|p| unnamed.contains(p));
            let new: Vec<PathBuf> = unnamed
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
                .filter(|p| self.read_asked.insert(p.clone()))
                .collect();
            if !new.is_empty() {
                let (tx, _) = self.reads.get_or_insert_with(std::sync::mpsc::channel);
                let tx = tx.clone();
                let (root, device) = (root.to_path_buf(), device_id.to_string());
                let tesseract = self.ingest.tesseract.clone();
                let ctx = ui.ctx().clone();
                let stop = self.read_stop.clone();
                let _ = std::thread::Builder::new()
                    .name("intake-form-reading".into())
                    .spawn(move || {
                        let pdfium = pdfium_dir().and_then(|d| filer_pdf::pdf::pdfium(&d).ok());
                        let ocr = vf_ocr::Tesseract::locate(tesseract.as_deref()).ok();
                        for p in new {
                            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }
                            // A broken PDF must not end the reading of the files after it.
                            let got =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    pdfium.and_then(|pdfium| {
                                        vf_ingest::recognize::read_inbox_file(
                                            pdfium,
                                            &root,
                                            &p,
                                            ocr.as_ref().map(|t| t as &dyn vf_ocr::OcrEngine),
                                            &device,
                                        )
                                    })
                                }))
                                .unwrap_or(None);
                            if tx.send((p, got)).is_err() {
                                return;
                            }
                            ctx.request_repaint();
                        }
                    });
            }
            if let Some((_, rx)) = &self.reads {
                while let Ok((p, got)) = rx.try_recv() {
                    if let Some(r) = got {
                        f.suggest(
                            &p,
                            r.client_ext_id.as_deref(),
                            r.year,
                            &format!("Read from the form: {}", r.note),
                        );
                    }
                }
            }
        }
        self.new_client_dialog(ui.ctx(), root, conn, device_id);
        match (&mut self.filer, &self.error) {
            (Some(f), _) => {
                eframe::App::ui(f, ui, frame);
                // Taken in the frame the double-click happened (the app opens it below).
                if let Some(p) = f.take_open_request() {
                    self.ingest.open_request = Some(p);
                }
            }
            (None, Some(e)) => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Intake");
                    ui.label(egui::RichText::new(e).color(crate::ui::theme::error()));
                });
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client IDs of letters and digits match; Filer's digits-only default is replaced once,
    /// a pattern the firm set is kept.
    #[test]
    fn id_pattern_takes_letters_and_digits_unless_the_firm_set_one() {
        assert!(filer_core::parse::validate_id_pattern(ID_PATTERN).is_ok());
        let re = filer_core::parse::filename_regex(ID_PATTERN).unwrap();
        let c = re
            .captures("Anderson Farms LLC_ANDE1042_2025 UltraTax CS Return.pdf")
            .unwrap();
        assert_eq!(&c["id"], "ANDE1042");
        assert_eq!(
            &re.captures("Smith John_1042_2025 1040.pdf").unwrap()["id"],
            "1042"
        );

        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let cabinet = CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: None,
        };
        vf_fs::cabinet::create(root, &cabinet).unwrap();
        let (s, _) = settings_with_theme(root, &cabinet, Some(false), 1.0).unwrap();
        assert_eq!(s.id_pattern, ID_PATTERN);
        assert!(
            !s.ultratax.enabled,
            "Filer's UltraTax reading is off: Vibe's print capture reads those names"
        );
        let dir = vf_fs::clients::master_dir(root);
        let (mut own, _) = Settings::load_or_default(&dir);
        assert_eq!(own.id_pattern, ID_PATTERN, "saved");
        own.id_pattern = r"\d{5}".into();
        own.save(&dir).unwrap();
        assert_eq!(
            settings_with_theme(root, &cabinet, Some(false), 1.0)
                .unwrap()
                .0
                .id_pattern,
            r"\d{5}"
        );
        // This PC's look reaches Filer's screen but not the cabinet's shared config.json.
        let before = std::fs::read(dir.join("config.json")).unwrap();
        let (s, _) = settings_with_theme(root, &cabinet, Some(true), 1.35).unwrap();
        assert_eq!(s.display.theme, ThemeChoice::Dark);
        assert!((s.display.zoom - 1.35).abs() < 0.001);
        assert_eq!(std::fs::read(dir.join("config.json")).unwrap(), before);
    }
}
