//! Import tab (UI_SPEC §8, P07b): zips that arrived in an inbox or were added by hand become
//! drafts. Each draft is pre-scanned, then either unpacked into a client folder keeping its
//! structure, or sent to the queue for the normal rules. Recent imports on the right.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use uuid::Uuid;

use crate::app::AppState;
use crate::clients::Client;
use crate::commit::{self, CommitTarget, UnpackEntry, UnpackInput, UnpackReport};
use crate::config::Level;
use crate::intake::QueuedFile;
use crate::oplog::{self, Op, OpRecord, OpResult};
use crate::plan::{CollisionChoice, FolderAction};
use crate::sentinel::Status as FolderStatus;
use crate::template;
use crate::ui::theme::{
    self, PillKind, banner, file_name, human_size, label_caps, mono, pal, pill, primary_button,
    strong,
};
use crate::ui::widgets::{search_clients, show_in_explorer};
use crate::worker;
use crate::zipimport::{self, EntryKind, PreScan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Unpack,
    Queue,
}

enum Scan {
    Loading(Receiver<Result<PreScan, String>>),
    Ready(Arc<PreScan>),
    Failed(String),
}

/// What the user chose for one draft.
struct Draft {
    scan: Scan,
    mode: Mode,
    client_query: String,
    client: Option<Client>,
    subfolder: String,
    /// Collision choice per entry index (others use `default_choice`).
    choices: HashMap<usize, CollisionChoice>,
    default_choice: CollisionChoice,
}

/// Collisions for one (zip, destination) — entry index, path, size of the existing file.
type Collisions = Vec<(usize, String, u64)>;

/// Send to queue: the flat extraction running.
struct QueueJob {
    rx: Receiver<Result<zipimport::Extracted, String>>,
    zip: PathBuf,
    id: Uuid,
    tmp: PathBuf,
}

struct UnpackJob {
    rx: Receiver<UnpackReport>,
    progress: Arc<AtomicUsize>,
    total: usize,
    zip: PathBuf,
}

#[derive(Default)]
pub struct ImportScreen {
    selected: Option<PathBuf>,
    drafts: HashMap<PathBuf, Draft>,
    /// Destination key → collisions, and the job computing them.
    collisions: Option<(String, Collisions)>,
    collisions_job: Option<(String, Receiver<Collisions>)>,
    /// Existing sub-folders of a client folder (key: folder path).
    subfolders: Option<(PathBuf, Vec<String>)>,
    subfolders_job: Option<(PathBuf, Receiver<Vec<String>>)>,
    job: Option<UnpackJob>,
    queue_job: Option<QueueJob>,
    message: Option<(Level, String)>,
    /// Recent zip_import records, newest first, and the log generation they were read at.
    recent: Vec<OpRecord>,
    undone: Vec<Uuid>,
    recent_generation: Option<u64>,
    recent_job: Option<Receiver<(Vec<OpRecord>, Vec<Uuid>)>>,
    picked_recent: Option<Uuid>,
    /// Size of data/tmp for the footer, when it was measured, and the job measuring it.
    tmp_size: Option<(std::time::Instant, u64)>,
    tmp_job: Option<Receiver<u64>>,
    /// Refused zips already logged as rejected this session.
    rejected_logged: std::collections::HashSet<PathBuf>,
    /// Refused zips waiting for Recent imports to be read before they are logged.
    pending_rejected: Vec<(PathBuf, String, bool)>,
}

/// Bytes in the files under `dir` (not following links).
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

impl ImportScreen {
    /// Footer slot (UI_SPEC §2): `data\tmp · 61 MB`.
    pub fn footer(&self) -> String {
        match self.tmp_size {
            Some((_, bytes)) => format!("data\\tmp · {}", human_size(bytes)),
            None => String::new(),
        }
    }

    /// Measures data/tmp every few seconds while the tab is open.
    fn measure_tmp(&mut self, ctx: &egui::Context, state: &AppState) {
        if let Some(rx) = &self.tmp_job
            && let Ok(bytes) = rx.try_recv()
        {
            self.tmp_size = Some((std::time::Instant::now(), bytes));
            self.tmp_job = None;
        }
        let stale = self
            .tmp_size
            .is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(5));
        if stale && self.tmp_job.is_none() {
            let dir = state.data_dir.join(crate::pdf::TMP_DIR);
            self.tmp_job = Some(worker::background(ctx, move || dir_size(&dir)));
        }
    }

    /// Drafts waiting (the tab badge).
    pub fn draft_count(state: &AppState) -> usize {
        state.intake.zips().len()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let ctx = ui.ctx().clone();
        if let Some(batch) = state.import_show.take() {
            self.picked_recent = Some(batch);
        }
        self.measure_tmp(&ctx, state);
        self.poll(&ctx, state);
        ui.add_space(4.0);
        if let Some((level, msg)) = &self.message {
            banner(ui, *level, msg);
        }
        let zips: Vec<QueuedFile> = state.intake.zips().to_vec();
        if self
            .selected
            .as_ref()
            .is_none_or(|s| !zips.iter().any(|z| &z.path == s))
        {
            self.selected = zips.first().map(|z| z.path.clone());
        }
        for z in &zips {
            self.drafts.entry(z.path.clone()).or_insert_with(|| {
                let p = z.path.clone();
                Draft {
                    scan: Scan::Loading(worker::background(&ctx, move || zipimport::prescan(&p))),
                    mode: Mode::Unpack,
                    client_query: String::new(),
                    client: None,
                    subfolder: String::new(),
                    choices: HashMap::new(),
                    default_choice: CollisionChoice::KeepBoth,
                }
            });
        }
        ui.horizontal_top(|ui| {
            let height = ui.available_height();
            ui.allocate_ui_with_layout(
                egui::vec2(640.0_f32.min(ui.available_width() * 0.58), height),
                Layout::top_down(Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("import_left")
                        .auto_shrink(false)
                        .show(ui, |ui| self.left_ui(ui, state, &zips));
                },
            );
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                self.recent_ui(ui, state, &zips);
            });
        });
    }

    fn poll(&mut self, ctx: &egui::Context, state: &mut AppState) {
        let mut refused: Vec<(PathBuf, String, bool)> = Vec::new();
        for d in self.drafts.values_mut() {
            if let Scan::Loading(rx) = &d.scan
                && let Ok(result) = rx.try_recv()
            {
                d.scan = match result {
                    Ok(s) => {
                        if let Some(why) = &s.refused {
                            refused.push((s.path.clone(), why.clone(), s.encrypted));
                        }
                        Scan::Ready(Arc::new(s))
                    }
                    Err(e) => Scan::Failed(e),
                };
            }
        }
        // A refused zip (encrypted, over the limits) shows in Recent imports as rejected, once:
        // it stays a draft and is pre-scanned again at every launch. Waits for Recent imports
        // to be read so an earlier rejection is seen.
        self.pending_rejected.extend(refused);
        let recent_read = self.recent_generation.is_some() && self.recent_job.is_none();
        let refused = if recent_read {
            std::mem::take(&mut self.pending_rejected)
        } else {
            Vec::new()
        };
        for (zip, why, encrypted) in refused {
            let logged = self.recent.iter().any(|r| {
                r.src.as_ref() == Some(&zip)
                    && r.note.as_deref().is_some_and(|n| n.starts_with("rejected"))
            });
            if logged || !self.rejected_logged.insert(zip.clone()) {
                continue;
            }
            let short = if encrypted {
                "encrypted"
            } else {
                "over the limits"
            };
            let rec = OpRecord {
                src: Some(zip),
                result: OpResult::Error,
                error: Some(why),
                note: Some(format!("rejected · {short}")),
                ..OpRecord::new(Uuid::new_v4(), crate::oplog::Op::ZipImport)
            };
            if let Err(e) = crate::oplog::append(&crate::oplog::path(&state.data_dir), &rec) {
                tracing::error!(error = %e, "cannot append to ops.jsonl");
            }
            state.ops_generation += 1;
        }
        if let Some((key, rx)) = &self.collisions_job
            && let Ok(c) = rx.try_recv()
        {
            self.collisions = Some((key.clone(), c));
            self.collisions_job = None;
        }
        if let Some((key, rx)) = &self.subfolders_job
            && let Ok(list) = rx.try_recv()
        {
            self.subfolders = Some((key.clone(), list));
            self.subfolders_job = None;
        }
        if let Some(job) = &self.job
            && let Ok(report) = job.rx.try_recv()
        {
            let zip = job.zip.clone();
            self.job = None;
            self.finish_unpack(ctx, state, zip, report);
        }
        if let Some(job) = &self.queue_job
            && let Ok(result) = job.rx.try_recv()
        {
            let (zip, id, tmp) = (job.zip.clone(), job.id, job.tmp.clone());
            self.queue_job = None;
            match result {
                Ok(files) => {
                    let n = files.len();
                    let queued: Vec<QueuedFile> = files
                        .into_iter()
                        .map(|(path, entry, size)| QueuedFile {
                            kind: crate::intake::kind_of(&path),
                            source: crate::intake::Source::Zip {
                                zip: zip.clone(),
                                entry,
                            },
                            path,
                            size,
                            added: chrono::Local::now(),
                        })
                        .collect();
                    let listed = state.intake.zips().iter().find(|q| q.path == zip).cloned();
                    state.routed.push(crate::app::RoutedZip {
                        id,
                        zip: zip.clone(),
                        listed,
                        tmp,
                        entries: queued.iter().map(|q| q.path.clone()).collect(),
                        files: queued.clone(),
                        filed: Vec::new(),
                    });
                    state.intake.add_routed(queued);
                    state.intake.drop_zip(&zip);
                    self.drafts.remove(&zip);
                    self.message = Some((
                        Level::Info,
                        format!(
                            "Queued {n} file{} from {}. They are in Preview; the zip goes once they are all filed.",
                            if n == 1 { "" } else { "s" },
                            file_name(&zip)
                        ),
                    ));
                }
                Err(e) => {
                    self.message =
                        Some((Level::Error, format!("Could not unpack to the queue: {e}")));
                }
            }
        }
        if let Some(rx) = &self.recent_job
            && let Ok((recent, undone)) = rx.try_recv()
        {
            self.recent = recent;
            self.undone = undone;
            self.recent_job = None;
        }
        if self.recent_job.is_none() && self.recent_generation != Some(state.ops_generation) {
            self.recent_generation = Some(state.ops_generation);
            let path = oplog::path(&state.data_dir);
            self.recent_job = Some(worker::background(ctx, move || {
                let records = oplog::read(&path).map(|r| r.records).unwrap_or_default();
                let undone: Vec<Uuid> = records
                    .iter()
                    .filter(|r| r.op == Op::Undo)
                    .map(|r| r.batch)
                    .collect();
                let mut recent: Vec<OpRecord> = records
                    .into_iter()
                    .filter(|r| r.op == Op::ZipImport)
                    .collect();
                recent.reverse();
                recent.truncate(50);
                (recent, undone)
            }));
        }
    }

    fn finish_unpack(
        &mut self,
        ctx: &egui::Context,
        state: &mut AppState,
        zip: PathBuf,
        r: UnpackReport,
    ) {
        state.ops_generation += 1;
        state.last_batch = Some(r.last_batch.clone());
        if r.folders_changed {
            state.start_scan(ctx);
        }
        let mut msg = format!(
            "Unpacked {} file{}",
            r.written,
            if r.written == 1 { "" } else { "s" }
        );
        if r.skipped > 0 {
            msg.push_str(&format!(", skipped {}", r.skipped));
        }
        msg.push('.');
        let level = if r.stopped.is_some() || !r.errors.is_empty() || !r.warnings.is_empty() {
            for e in r.stopped.iter().chain(&r.errors).chain(&r.warnings) {
                msg.push_str(&format!(" {e}."));
            }
            Level::Error
        } else {
            Level::Info
        };
        self.message = Some((level, msg));
        if r.zip_deleted {
            state.intake.drop_zip(&zip);
            self.drafts.remove(&zip);
        }
        self.picked_recent = Some(r.last_batch.batch);
    }

    fn left_ui(&mut self, ui: &mut egui::Ui, state: &mut AppState, zips: &[QueuedFile]) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            match self
                .selected
                .as_ref()
                .and_then(|s| zips.iter().find(|z| &z.path == s))
            {
                Some(z) => {
                    let from = match &z.source {
                        crate::intake::Source::Inbox(i) => format!("arrived in {}", i.display()),
                        _ => "added by hand".into(),
                    };
                    ui.label(strong("Draft import"));
                    ui.label(
                        RichText::new(format!(" · {from} {}", {
                            // "today 4:51 PM" for a same-day arrival (UI_SPEC §8).
                            if z.added.date_naive() == chrono::Local::now().date_naive() {
                                z.added.format("today %-I:%M %p").to_string()
                            } else {
                                z.added.format("%b %-d, %-I:%M %p").to_string()
                            }
                        }))
                        .size(12.0)
                        .color(p.muted),
                    );
                }
                None => {
                    ui.label(strong("No zips waiting"));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Browse for zip…").clicked()
                    && let Some(files) = rfd::FileDialog::new()
                        .add_filter("Zip", &["zip"])
                        .pick_files()
                {
                    state.intake.add_paths(files);
                }
            });
        });
        if zips.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for z in zips {
                    let name = z
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let on = self.selected.as_ref() == Some(&z.path);
                    if ui.selectable_label(on, name).clicked() {
                        self.selected = Some(z.path.clone());
                    }
                }
            });
        }
        ui.add_space(6.0);
        let Some(path) = self.selected.clone() else {
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(
                        "Zips that land in a watched inbox, or are dropped on the window, show up here.",
                    )
                    .color(p.muted),
                );
            });
            return;
        };
        let scan = match self.drafts.get(&path).map(|d| &d.scan) {
            Some(Scan::Ready(s)) => s.clone(),
            Some(Scan::Failed(e)) => {
                let e = e.clone();
                theme::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(strong(file_name(&path)));
                        pill(ui, PillKind::Err, e.as_str());
                    });
                });
                self.action_row(ui, state, &path, None);
                return;
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the zip…");
                });
                return;
            }
        };
        zip_card(ui, &path, &scan);
        if scan.refused.is_some() {
            self.action_row(ui, state, &path, None);
            return;
        }
        ui.add_space(8.0);
        self.mode_cards(ui, &path, &scan);
        let mode = self.drafts.get(&path).map_or(Mode::Unpack, |d| d.mode);
        let dest = if mode == Mode::Unpack {
            ui.add_space(8.0);
            self.destination_card(ui, state, &path)
        } else {
            None
        };
        if let Some(dest) = &dest {
            ui.add_space(8.0);
            self.collisions_card(ui, &path, &scan, dest);
        }
        ui.add_space(8.0);
        self.action_row(ui, state, &path, Some((&scan, dest)));
    }

    fn mode_cards(&mut self, ui: &mut egui::Ui, path: &Path, scan: &PreScan) {
        let Some(d) = self.drafts.get_mut(path) else {
            return;
        };
        let n = scan.importable().count();
        ui.columns(2, |cols| {
            for (col, mode, title, text) in [
                (
                    0,
                    Mode::Unpack,
                    "Unpack to a client folder",
                    "Keeps the zip's folder structure. Any file type. You pick the client."
                        .to_string(),
                ),
                (
                    1,
                    Mode::Queue,
                    "Send contents to the queue",
                    format!("Extracts flat, then the normal rules flow in Preview. {n} files."),
                ),
            ] {
                let ui = &mut cols[col];
                let p = pal(ui);
                let on = d.mode == mode;
                let resp = Frame::new()
                    .fill(if on {
                        p.accent.gamma_multiply(0.06)
                    } else {
                        p.card
                    })
                    .stroke(Stroke::new(
                        if on { 1.5 } else { 1.0 },
                        if on { p.accent } else { p.border },
                    ))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut sel = on;
                            if ui.radio_value(&mut sel, true, strong(title)).clicked() {
                                d.mode = mode;
                            }
                        });
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.label(RichText::new(text).size(12.0).color(p.muted));
                        });
                    })
                    .response;
                if ui
                    .interact(
                        resp.rect,
                        egui::Id::unique(("mode_card", col)),
                        egui::Sense::click(),
                    )
                    .clicked()
                {
                    d.mode = mode;
                }
            }
        });
    }

    /// Client, sub-folder, where it lands. Returns the destination when it is known.
    fn destination_card(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut AppState,
        path: &Path,
    ) -> Option<Dest> {
        let p = pal(ui);
        let Some(list) = state.clients.list().cloned() else {
            // Without a client list there is nothing to pick from: say so instead of hiding the
            // card (owner 2026-11-02).
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                label_caps(ui, "Unpack destination");
                ui.add_space(4.0);
                let text = if state.clients.busy() {
                    "Loading the client list…"
                } else {
                    "No client list yet. Import the client CSV in Setup (Client master list), then pick the client here."
                };
                ui.label(RichText::new(text).color(p.muted));
            });
            return None;
        };
        let root = state.settings.root.clone();
        let mut result = None;
        let mut want_subfolders: Option<PathBuf> = None;
        let subfolders = self.subfolders.clone();
        let d = self.drafts.get_mut(path)?;
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            label_caps(ui, "Unpack destination");
            ui.add_space(4.0);
            egui::Grid::new("import_dest").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                let l = ui.label("Client");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut d.client_query)
                                .hint_text("ID or name")
                                .desired_width(220.0),
                        );
                        let field = field.labelled_by(l.id);
                        if field.changed() {
                            d.client = list.match_by_id(d.client_query.trim()).cloned();
                        }
                        match &d.client {
                            Some(c) if c.id == d.client_query.trim() => {
                                pill(ui, PillKind::Ok, format!("{} · exact", c.id));
                            }
                            Some(c) => {
                                pill(ui, PillKind::Info, c.id.clone());
                            }
                            None => {}
                        }
                        ui.label(RichText::new("typed, never guessed from the zip name").size(11.0).color(p.muted));
                    });
                    if d.client.is_none() && !d.client_query.trim().is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for (score, c) in search_clients(&list, &d.client_query, 6) {
                                let text = match score {
                                    Some(s) => format!("{} {} · {s:.2}", c.name, c.id),
                                    None => format!("{} {} · ID match", c.name, c.id),
                                };
                                if ui.small_button(text).clicked() {
                                    d.client_query = c.name.clone();
                                    d.client = Some(c.clone());
                                }
                            }
                        });
                    } else if let Some(c) = &d.client {
                        ui.label(RichText::new(&c.name).size(12.0).color(p.muted));
                    }
                });
                ui.end_row();

                let l = ui.label("Subfolder");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut d.subfolder)
                                .hint_text("blank = client root")
                                .desired_width(220.0),
                        )
                        .labelled_by(l.id);
                        ui.label(RichText::new("blank = client root").size(11.0).color(p.muted));
                    });
                    if let (Some(c), Some(root)) = (&d.client, &root) {
                        let folder = client_folder(state, root, c);
                        if let Some(f) = &folder {
                            let key = f.path().to_path_buf();
                            match &subfolders {
                                Some((k, names)) if *k == key => {
                                    let typed = d.subfolder.trim().to_lowercase();
                                    ui.horizontal_wrapped(|ui| {
                                        for n in names.iter().filter(|n| typed.is_empty() || n.to_lowercase().contains(&typed)).take(8) {
                                            if ui.small_button(n).clicked() {
                                                d.subfolder = n.clone();
                                            }
                                        }
                                    });
                                }
                                _ => want_subfolders = Some(key),
                            }
                        }
                    }
                });
                ui.end_row();

                ui.label("Lands in");
                match (&d.client, &root) {
                    (Some(c), Some(root)) => match client_folder(state, root, c) {
                        Some(folder) => match safe_subfolder(&d.subfolder) {
                            Ok(sub) => {
                                let dir = folder.path().join(&sub);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(mono(dir.display().to_string()).color(p.text));
                                    if let FolderAction::WillCreate(_) = folder {
                                        pill(ui, PillKind::Info, "folder will be created");
                                    }
                                });
                                result = Some(Dest { folder, dir, client: c.clone() });
                            }
                            Err(e) => {
                                ui.label(RichText::new(e).color(p.pill(PillKind::Err).1));
                            }
                        },
                        None => {
                            ui.label(RichText::new("The client's folder name can't be built; check the template.").color(p.pill(PillKind::Err).1));
                        }
                    },
                    (_, None) => {
                        ui.label(RichText::new("Set the client root in Setup.").color(p.muted));
                    }
                    _ => {
                        ui.label(RichText::new("Pick the client first.").color(p.muted));
                    }
                }
                ui.end_row();
            });
        });
        if let Some(key) = want_subfolders
            && self.subfolders_job.as_ref().is_none_or(|(k, _)| *k != key)
        {
            let k = key.clone();
            self.subfolders_job = Some((
                key,
                worker::background(ui.ctx(), move || list_subfolders(&k)),
            ));
        }
        result
    }

    fn collisions_card(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        scan: &Arc<PreScan>,
        dest: &Dest,
    ) {
        let p = pal(ui);
        let key = format!("{}|{}", path.display(), dest.dir.display());
        let ready = self
            .collisions
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, c)| c.clone());
        if ready.is_none() && self.collisions_job.as_ref().is_none_or(|(k, _)| *k != key) {
            let (scan, dir) = (scan.clone(), dest.dir.clone());
            let k = key.clone();
            self.collisions_job = Some((
                key.clone(),
                worker::background(ui.ctx(), move || {
                    scan.importable()
                        .filter_map(|e| {
                            let rel = e.path.clone()?;
                            let target = dir.join(rel.replace('/', "\\"));
                            std::fs::metadata(&target)
                                .ok()
                                .map(|m| (e.index, rel, m.len()))
                        })
                        .collect()
                }),
            ));
            let _ = k;
        }
        let Some(d) = self.drafts.get_mut(path) else {
            return;
        };
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                label_caps(ui, "Collisions");
                ui.label(
                    RichText::new("checked before anything extracts")
                        .size(12.0)
                        .color(p.muted),
                );
                if let Some(c) = &ready
                    && !c.is_empty()
                {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("import_collision_default")
                            .selected_text(d.default_choice.label())
                            .show_ui(ui, |ui| {
                                for c in CollisionChoice::ALL {
                                    ui.selectable_value(&mut d.default_choice, c, c.label());
                                }
                            });
                        ui.label(RichText::new("For all").size(12.0).color(p.muted));
                    });
                }
            });
            match &ready {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking the destination…");
                    });
                }
                Some(c) if c.is_empty() => {
                    ui.label(
                        RichText::new("None: no file in the zip already exists there.")
                            .color(p.muted),
                    );
                }
                Some(c) => {
                    for (index, rel, size) in c.iter().take(200) {
                        ui.horizontal(|ui| {
                            ui.label(mono(rel).color(p.text));
                            ui.label(
                                RichText::new(format!("{} exists", human_size(*size)))
                                    .size(12.0)
                                    .color(p.muted),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let mut choice =
                                    d.choices.get(index).copied().unwrap_or(d.default_choice);
                                let options = [
                                    (CollisionChoice::Overwrite, "Overwrite"),
                                    (CollisionChoice::KeepBoth, "Keep both"),
                                    (CollisionChoice::Skip, "Skip"),
                                ];
                                if theme::segmented(ui, &mut choice, &options) {
                                    d.choices.insert(*index, choice);
                                }
                            });
                        });
                    }
                }
            }
        });
    }

    fn action_row(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut AppState,
        path: &Path,
        ready: Option<(&Arc<PreScan>, Option<Dest>)>,
    ) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(
                    "Each file is written to a .tmp, hashed, then renamed. A copy of the zip is kept in data\\tmp\\undo\\ for undo.",
                )
                .size(11.0)
                .color(p.muted),
            );
        });
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                let done = job.progress.load(Ordering::Relaxed);
                ui.add(
                    egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                        .desired_width(240.0)
                        .text(format!("Unpacking… {done}/{}", job.total)),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // An unpack or a queue extraction running: no second action on any zip.
                let busy = self.job.is_some() || self.queue_job.is_some();
                let mode = self.drafts.get(path).map_or(Mode::Unpack, |d| d.mode);
                if let Some((scan, dest)) = ready {
                    let n = scan.importable().count();
                    match mode {
                        Mode::Unpack => {
                            let key = dest
                                .as_ref()
                                .map(|d| format!("{}|{}", path.display(), d.dir.display()));
                            let checked = key.is_some()
                                && self
                                    .collisions
                                    .as_ref()
                                    .is_some_and(|(k, _)| Some(k) == key.as_ref());
                            let can = !busy && dest.is_some() && checked && n > 0;
                            let b = ui
                                .add_enabled(can, primary_button(ui, format!("Unpack {n} files")));
                            let b = if dest.is_none() {
                                b.on_disabled_hover_text("Pick the client first.")
                            } else {
                                b
                            };
                            if b.clicked()
                                && let Some(dest) = dest
                            {
                                self.start_unpack(ui.ctx(), state, path, scan, dest);
                            }
                        }
                        Mode::Queue => {
                            if ui
                                .add_enabled(
                                    !busy && n > 0,
                                    primary_button(ui, format!("Queue {n} files")),
                                )
                                .clicked()
                            {
                                let id = Uuid::new_v4();
                                let tmp = state
                                    .data_dir
                                    .join(crate::pdf::TMP_DIR)
                                    .join(format!("zip-{}", id.simple()));
                                let (s, t) = (scan.clone(), tmp.clone());
                                self.queue_job = Some(QueueJob {
                                    rx: worker::background(ui.ctx(), move || {
                                        zipimport::extract_flat(&s, &t)
                                    }),
                                    zip: path.to_path_buf(),
                                    id,
                                    tmp,
                                });
                            }
                            if self.queue_job.is_some() {
                                ui.spinner();
                            }
                        }
                    }
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Discard draft"))
                    .on_hover_text("Leaves the zip where it is")
                    .clicked()
                {
                    state.intake.drop_zip(path);
                    self.drafts.remove(path);
                    self.selected = None;
                }
            });
        });
    }

    fn start_unpack(
        &mut self,
        ctx: &egui::Context,
        state: &AppState,
        path: &Path,
        scan: &PreScan,
        dest: Dest,
    ) {
        let Some(root) = state.settings.root.clone() else {
            return;
        };
        let Some(d) = self.drafts.get(path) else {
            return;
        };
        let entries: Vec<UnpackEntry> = scan
            .importable()
            .filter_map(|e| {
                let rel = e.path.clone()?;
                Some(UnpackEntry {
                    index: e.index,
                    dest: dest.dir.join(rel.replace('/', "\\")),
                    choice: d.choices.get(&e.index).copied().unwrap_or(d.default_choice),
                    rel,
                })
            })
            .collect();
        let input = UnpackInput {
            zip: path.to_path_buf(),
            client: dest.client.clone(),
            folder: dest.folder.clone(),
            dest_dir: dest.dir.clone(),
            left_out: left_out(scan),
            entries,
            target: CommitTarget {
                root,
                first_inbox: state.settings.inboxes.first().cloned(),
                data_dir: state.data_dir.clone(),
                firm_id: state.settings.firm_id.unwrap_or_default(),
                template: state.template.clone(),
            },
        };
        let progress = Arc::new(AtomicUsize::new(0));
        let total = input.entries.len();
        let p = progress.clone();
        self.message = None;
        self.job = Some(UnpackJob {
            rx: worker::background(ctx, move || commit::run_unpack(&input, &p)),
            progress,
            total,
            zip: path.to_path_buf(),
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn recent_ui(&mut self, ui: &mut egui::Ui, state: &mut AppState, zips: &[QueuedFile]) {
        let p = pal(ui);
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(strong("Recent imports"));
            ui.add_space(4.0);
            for z in zips {
                Frame::new()
                    .fill(p.tint_attention)
                    .inner_margin(Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(mono(file_name(&z.path)).color(p.text));
                            pill(ui, PillKind::Warn, "draft");
                        });
                    });
            }
            if self.recent.is_empty() && zips.is_empty() {
                ui.label(RichText::new("No zips imported yet.").color(p.muted));
            }
            let recent = self.recent.clone();
            for r in &recent {
                let picked = self.picked_recent == Some(r.batch);
                let row = Frame::new()
                    .fill(if picked {
                        p.accent.gamma_multiply(0.08)
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .inner_margin(Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let zip = r.src.as_deref().map(file_name).unwrap_or_default();
                            ui.label(mono(zip).color(p.text));
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · {}",
                                    import_mode(r),
                                    r.ts.with_timezone(&chrono::Local).format("%b %-d"),
                                    r.client_id.clone().unwrap_or_default()
                                ))
                                .size(11.0)
                                .color(p.muted),
                            );
                            for (kind, text) in result_pills(r, self.undone.contains(&r.batch)) {
                                pill(ui, kind, text);
                            }
                        });
                    });
                if ui
                    .interact(
                        row.response.rect,
                        egui::Id::unique(("recent", r.batch)),
                        egui::Sense::click(),
                    )
                    .clicked()
                {
                    self.picked_recent = Some(r.batch);
                }
            }
        });
        if let Some(r) = self
            .recent
            .iter()
            .find(|r| Some(r.batch) == self.picked_recent)
            .cloned()
        {
            ui.add_space(8.0);
            theme::card(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("import_detail")
                    .num_columns(2)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        label_caps(ui, "Batch");
                        ui.label(format!(
                            "{} · {}",
                            &r.batch.simple().to_string()[..8],
                            r.ts.with_timezone(&chrono::Local)
                                .format("%b %-d, %-I:%M %p")
                        ));
                        ui.end_row();
                        label_caps(ui, "Destination");
                        ui.label(
                            mono(
                                r.dst
                                    .as_ref()
                                    .map(|d| d.display().to_string())
                                    .unwrap_or_default(),
                            )
                            .color(p.text),
                        );
                        ui.end_row();
                        label_caps(ui, "Entries");
                        ui.label(r.note.clone().unwrap_or_default());
                        ui.end_row();
                        label_caps(ui, "Source zip");
                        let kept = state
                            .last_batch
                            .as_ref()
                            .filter(|lb| lb.batch == r.batch)
                            .and_then(|lb| {
                                let src = r.src.as_ref()?;
                                lb.zip
                                    .iter()
                                    .chain(&lb.routed_zips)
                                    .find(|z| &z.original == src)
                            })
                            .map(|z| z.kept.clone());
                        ui.label(match kept {
                            Some(k) if k.exists() => {
                                format!("deleted; undo copy at {}", k.display())
                            }
                            _ if r.note.as_deref().is_some_and(|n| n.contains("zip deleted")) => {
                                "deleted".to_string()
                            }
                            _ => "kept where it was".to_string(),
                        });
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    if let Some(d) = &r.dst
                        && ui.button("Open folder").clicked()
                    {
                        show_in_explorer(d);
                    }
                    if ui.button("Show in History").clicked() {
                        state.history_filter = Some(r.batch);
                        state.goto = Some(crate::app::Tab::History);
                    }
                });
            });
        }
    }
}

/// Where an Unpack goes.
#[derive(Debug, Clone)]
struct Dest {
    folder: FolderAction,
    dir: PathBuf,
    client: Client,
}

/// The client's bound folder, an ID-named folder to bind, or a folder to create from the template.
fn client_folder(state: &AppState, root: &Path, client: &Client) -> Option<FolderAction> {
    if let Some(scan) = state.folders.scan_of(Some(root)) {
        let mine = |b: &&crate::sentinel::FolderBinding| {
            b.client_id.as_deref() == Some(client.id.as_str())
        };
        if let Some(b) = scan
            .folders
            .iter()
            .filter(mine)
            .find(|b| b.status == FolderStatus::Bound)
        {
            return Some(FolderAction::Exists(b.path.clone()));
        }
        if let Some(b) = scan
            .folders
            .iter()
            .filter(mine)
            .find(|b| b.status == FolderStatus::UnboundNamed)
        {
            return Some(FolderAction::WillBind(b.path.clone()));
        }
    }
    let name = template::client_folder_name(&state.template, client).ok()?;
    let path = root.join(name);
    Some(if path.is_dir() {
        // Exists but not bound: treat as binding it (the scan has not seen it yet).
        FolderAction::WillBind(path)
    } else {
        FolderAction::WillCreate(path)
    })
}

/// The typed sub-folder as a relative path; refuses anything that could leave the client folder.
fn safe_subfolder(s: &str) -> Result<PathBuf, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(PathBuf::new());
    }
    // The same rules as Preview's folder picker (no .., drives, or names Windows can't hold).
    crate::plan::checked_rel(Path::new(&s.replace('/', "\\")))
        .map_err(|e| format!("That sub-folder isn't allowed: {e}."))
}

/// Folders below `dir`, two levels deep, as relative paths.
fn list_subfolders(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(top) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in top.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !p.is_dir() || name.starts_with(['_', '.', '~']) {
            continue;
        }
        out.push(name.clone());
        if let Ok(sub) = std::fs::read_dir(&p) {
            for s in sub.flatten() {
                if s.path().is_dir() {
                    out.push(format!("{name}\\{}", s.file_name().to_string_lossy()));
                }
            }
        }
    }
    out.sort_by_key(|n| n.to_lowercase());
    out
}

/// "2 junk · 1 blocked · 1 unsafe" for the summary.
fn left_out(scan: &PreScan) -> String {
    let mut parts = Vec::new();
    for (kind, word) in [
        (EntryKind::Junk, "junk"),
        (EntryKind::Blocked, "blocked"),
        (EntryKind::Unsafe, "unsafe"),
    ] {
        let n = scan.of_kind(kind).len();
        if n > 0 {
            parts.push(format!("{n} {word}"));
        }
    }
    parts.join(" · ")
}

/// Pills for a recent import: "118 written", "2 skipped", "undone", errors.
/// Unpack, Route (sent to the queue) or Rejected, from the zip_import record's note.
fn import_mode(r: &OpRecord) -> &'static str {
    match r.note.as_deref().unwrap_or("") {
        n if n.starts_with("queue") => "Route",
        n if n.starts_with("rejected") => "Rejected",
        _ => "Unpack",
    }
}

fn result_pills(r: &OpRecord, undone: bool) -> Vec<(PillKind, String)> {
    let mut out = Vec::new();
    if let Some(why) = r
        .note
        .as_deref()
        .and_then(|n| n.strip_prefix("rejected · "))
    {
        out.push((PillKind::Err, format!("{why} · rejected")));
        return out;
    }
    if r.result == OpResult::Error {
        out.push((
            PillKind::Err,
            r.error.clone().unwrap_or_else(|| "failed".into()),
        ));
    }
    if let Some(note) = &r.note {
        for part in note.split(" · ") {
            if part.ends_with("written") {
                out.push((PillKind::Ok, part.to_string()));
            } else if part.ends_with("skipped") || part.ends_with("queued") {
                out.push((PillKind::Neutral, part.to_string()));
            } else if part.ends_with("failed") {
                out.push((PillKind::Err, part.to_string()));
            }
        }
    }
    if undone {
        out.push((PillKind::Neutral, "undone".into()));
    }
    out
}

/// The zip card: name, sizes, pre-scan pill, and what the pre-scan found (UI_SPEC §8).
fn zip_card(ui: &mut egui::Ui, path: &Path, scan: &PreScan) {
    let p = pal(ui);
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(strong(file_name(path)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match &scan.refused {
                    Some(why) => {
                        pill(ui, PillKind::Err, format!("refused · {why}"));
                    }
                    None => {
                        pill(ui, PillKind::Ok, "pre-scan ok");
                    }
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "{} entries · {} uncompressed · {} on disk",
                scan.entries.len(),
                human_size(scan.total_bytes),
                human_size(scan.zip_bytes)
            ))
            .size(12.0)
            .color(p.muted),
        );
        ui.add_space(6.0);
        let names = |kind: EntryKind| {
            scan.of_kind(kind)
                .iter()
                .map(|e| e.path.clone().unwrap_or_else(|| e.raw_name.clone()))
                .take(3)
                .collect::<Vec<_>>()
                .join(", ")
        };
        egui::Grid::new("zip_scan")
            .num_columns(2)
            .spacing([14.0, 5.0])
            .show(ui, |ui| {
                label_caps(ui, "Top level");
                ui.label(mono(scan.top_level()).color(p.text));
                ui.end_row();
                for (kind, label, word, note) in [
                    (EntryKind::Junk, "Dropped", "junk", ""),
                    (
                        EntryKind::Blocked,
                        "Blocked",
                        "executable",
                        "never extracted",
                    ),
                    (EntryKind::Unsafe, "Unsafe path", "entry", "rejected"),
                    (
                        EntryKind::NestedZip,
                        "Nested zips",
                        "zip",
                        "imported as a file, not expanded",
                    ),
                ] {
                    let n = scan.of_kind(kind).len();
                    if n == 0 {
                        continue;
                    }
                    label_caps(ui, label);
                    ui.horizontal_wrapped(|ui| {
                        let pk = match kind {
                            EntryKind::Blocked | EntryKind::Unsafe => PillKind::Err,
                            EntryKind::Junk => PillKind::Neutral,
                            _ => PillKind::Info,
                        };
                        pill(
                            ui,
                            pk,
                            format!(
                                "{n} {word}{}",
                                if n == 1 || word == "junk" { "" } else { "s" }
                            ),
                        );
                        ui.label(mono(names(kind)).color(p.muted));
                        if !note.is_empty() {
                            ui.label(RichText::new(format!("· {note}")).size(12.0).color(p.muted));
                        }
                    });
                    ui.end_row();
                }
                label_caps(ui, "Guards");
                ui.label(
                    RichText::new(
                        "≤ 2,000 entries · ≤ 100 MB per entry · ≤ 1 GB total · not encrypted",
                    )
                    .size(12.0)
                    .color(p.muted),
                );
                ui.end_row();
            });
    });
}

#[cfg(test)]
mod tests {
    use super::safe_subfolder;
    use std::path::PathBuf;

    #[test]
    fn subfolder_follows_the_folder_picker_rules() {
        assert_eq!(safe_subfolder("  "), Ok(PathBuf::new()));
        assert_eq!(
            safe_subfolder("2024/Bank"),
            Ok(PathBuf::from("2024").join("Bank"))
        );
        for bad in [r"..\x", r"C:\x", "CON", "trailing.", "a<b"] {
            assert!(safe_subfolder(bad).is_err(), "{bad}");
        }
    }
}
