//! Vibe Files' own intake strip above the mounted Filer app (P02 steps 3–6): this PC's staging
//! folders and what the `vf_ingest` worker did with their files, plus the barcode sheets.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::browse::Snapshot;
use vf_ingest::pipeline::Origin;
use vf_ingest::worker::{self, Event, Worker};

use crate::config::Ingest;
use crate::ui::cover_sheet::{self, CoverSheetDialog};
use crate::ui::widgets::{self, DialogOutcome};
use crate::ui::{theme, wizard};

/// How often the worker's folders and Filer's inbox are looked up again.
const RECHECK: Duration = Duration::from_secs(3);
const LOG_MAX: usize = 6;

#[derive(Default)]
pub struct IngestState {
    worker: Option<Worker>,
    checked: Option<Instant>,
    /// Filer's first inbox, where processed files go.
    pub inbox: Option<PathBuf>,
    /// Why a staging folder is not watched.
    pub warning: Option<String>,
    /// What the worker did, newest first.
    pub log: Vec<String>,
    pub cover: Option<CoverSheetDialog>,
    /// A sheet PDF to open in the PDF app (the app runs it through Browse's shell).
    pub open_request: Option<PathBuf>,
    /// The staging folders changed here; the app saves `config.toml`.
    pub changed: Option<Ingest>,
    /// Files delivered to Filer's inbox since the cabinet opened.
    pub delivered: usize,
}

/// A staging folder must exist, be outside the cabinet, and not be one of Filer's inboxes
/// (Filer would file the unprocessed original too).
pub fn check_staging(dir: &Path, root: &Path, inboxes: &[PathBuf]) -> Result<(), String> {
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    let d = canon(dir).ok_or_else(|| format!("{} cannot be opened.", dir.display()))?;
    if !d.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    if let Some(r) = canon(root)
        && (d.starts_with(&r) || r.starts_with(&d))
    {
        return Err(format!(
            "{} overlaps the cabinet. Pick a folder outside it.",
            dir.display()
        ));
    }
    if inboxes.iter().filter_map(|i| canon(i)).any(|i| i == d) {
        return Err(format!(
            "{} is one of Filer's inboxes. Pick another folder: Vibe Files delivers the processed files to the inbox.",
            dir.display()
        ));
    }
    Ok(())
}

/// Filer's inbox folders for this cabinet (its `config.json`).
fn filer_inboxes(root: &Path) -> Vec<PathBuf> {
    filer_intake::config::Settings::load_or_default(&vf_fs::clients::master_dir(root))
        .0
        .inboxes
}

impl IngestState {
    /// Files waiting in the staging folders.
    pub fn waiting(&self) -> usize {
        self.worker.as_ref().map_or(0, Worker::waiting)
    }

    /// Keeps the worker pointed at this PC's folders and collects what it did. Every frame,
    /// whatever tab is showing.
    pub fn tick(&mut self, ctx: &egui::Context, root: &Path, device_id: &str, prefs: &Ingest) {
        if let Some(w) = &self.worker {
            for ev in w.events() {
                let line = match ev {
                    Event::Processed(p) => {
                        self.delivered += p.delivered.len();
                        let name = file_name(&p.staged);
                        let parts: Vec<String> = p
                            .delivered
                            .iter()
                            .map(|d| format!("{} ({})", file_name(&d.path), d.note))
                            .collect();
                        format!("{name} → {}", parts.join(" · "))
                    }
                    Event::Failed { staged, error } => {
                        format!("{} set aside in _failed: {error}", file_name(&staged))
                    }
                };
                self.log.insert(0, line);
                self.log.truncate(LOG_MAX);
            }
        }
        if self.checked.is_some_and(|t| t.elapsed() < RECHECK) {
            return;
        }
        self.checked = Some(Instant::now());
        let inboxes = filer_inboxes(root);
        self.inbox = inboxes.first().cloned();
        let mut warnings = Vec::new();
        let mut folders = Vec::new();
        for (dir, origin) in [
            (&prefs.scans, Origin::Scans),
            (&prefs.print_capture, Origin::PrintCapture),
        ] {
            if let Some(d) = dir {
                match check_staging(d, root, &inboxes) {
                    Ok(()) => folders.push((d.clone(), origin)),
                    Err(e) => warnings.push(e),
                }
            }
        }
        if self.inbox.is_none() && !folders.is_empty() {
            warnings.push(
                "Filer has no inbox yet: add one in Setup below. Processed files go to Filer's first inbox."
                    .into(),
            );
        }
        self.warning = (!warnings.is_empty()).then(|| warnings.join(" "));
        let cfg = worker::Config {
            root: root.to_path_buf(),
            inbox: self.inbox.clone(),
            folders,
            device_id: device_id.into(),
            pdfium_dir: crate::ui::intake::pdfium_dir(),
        };
        match &self.worker {
            Some(w) => w.set_config(cfg),
            None if !cfg.folders.is_empty() => {
                let ctx = ctx.clone();
                self.worker = Some(Worker::start(cfg, move || ctx.request_repaint()));
            }
            None => {}
        }
    }

    /// Looks again at the next tick (a folder was chosen).
    fn recheck(&mut self) {
        self.checked = None;
    }

    pub fn strip(
        &mut self,
        ui: &mut egui::Ui,
        prefs: &Ingest,
        snap: &Snapshot,
        sheets_dir: &Path,
        root: &Path,
    ) {
        egui::Panel::top("vf-ingest")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANE)
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Scans folder").strong());
                    let shown = prefs
                        .scans
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "not set".into());
                    ui.label(RichText::new(shown).monospace());
                    if ui
                        .button("Choose…")
                        .on_hover_text(
                            "Point the scanner and phone uploads here. Stacks are split at barcode sheets, then go to Filer's inbox.",
                        )
                        .clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        match check_staging(&dir, root, &filer_inboxes(root)) {
                            Ok(()) => {
                                let mut p = prefs.clone();
                                p.scans = Some(dir);
                                self.changed = Some(p);
                                self.recheck();
                            }
                            Err(e) => self.warning = Some(e),
                        }
                    }
                    ui.label(widgets::caption(format!("{} waiting", self.waiting())));
                    ui.separator();
                    ui.label(RichText::new("Print capture").strong());
                    let shown = prefs
                        .print_capture
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "off (Admin › Print capture & parsers)".into());
                    ui.label(RichText::new(shown).monospace());
                    ui.separator();
                    if ui.button("Cover sheet for client…").clicked() {
                        self.cover = Some(CoverSheetDialog::new(snap, None));
                    }
                    if ui.button("Print 10 blank separators").clicked() {
                        match cover_sheet::separators(sheets_dir) {
                            Ok(p) => self.open_request = Some(p),
                            Err(e) => self.warning = Some(e),
                        }
                    }
                });
                if let Some(w) = &self.warning {
                    ui.label(RichText::new(w).color(wizard::ERROR));
                }
                for l in self.log.iter().take(2) {
                    ui.label(widgets::caption(l));
                }
            });
        if let Some(d) = &mut self.cover {
            match cover_sheet::show(ui.ctx(), d, sheets_dir) {
                DialogOutcome::Open => {}
                DialogOutcome::Cancelled => self.cover = None,
                DialogOutcome::Done(p) => {
                    self.cover = None;
                    self.open_request = Some(p);
                }
            }
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
