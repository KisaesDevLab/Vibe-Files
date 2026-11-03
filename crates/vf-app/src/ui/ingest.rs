//! Vibe Files' own intake strip above the mounted Filer app (P02 steps 3–6): this PC's staging
//! folders and what the `vf_ingest` worker did with their files, plus the barcode sheets.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::RichText;
use vf_core::browse::Snapshot;
use vf_ingest::pipeline::Origin;
use vf_ingest::scanner::{self, Devices, Kind};
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
    /// This PC's Tesseract setting (`config.toml [ocr]`), for reading scanned forms (P08).
    pub tesseract: Option<PathBuf>,
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
    /// PDFs and zips in Filer's inboxes (what its queue holds), at the last check.
    in_inboxes: usize,
    /// This PC's scanners, once listed (`None` while listing).
    devices: Option<Devices>,
    devices_job: Option<Receiver<Devices>>,
    /// Files just delivered to Filer's inbox, to check against the catalog: (file, staging
    /// folder it came from, hashes to look up: as delivered and as it arrived).
    to_check: Vec<(PathBuf, PathBuf, [String; 2])>,
    /// Delivered files whose content is already in the cabinet (before Filer files them).
    pub pre_dupes: Vec<PreDup>,
    /// (index into `pre_dupes`, remove it from the inbox).
    pre_action: Option<(usize, bool)>,
    /// A scan running on its own thread.
    scan_job: Option<Receiver<Result<(PathBuf, scanner::Assembled), String>>>,
}

/// A file in Filer's inbox with the same content as a document already filed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreDup {
    pub path: PathBuf,
    /// The staging folder it came from (where "Remove" puts it, in `_duplicates`).
    pub staging: PathBuf,
    /// Cabinet-relative path of the document already holding the content.
    pub existing: String,
}

/// PDFs and zips Filer would queue from `inboxes` (sub-folders too, not `_combined`).
pub fn count_queued(inboxes: &[PathBuf]) -> usize {
    fn walk(dir: &Path, depth: usize) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.filter_map(Result::ok)
            .map(|e| {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with('.') || name.starts_with("~$") {
                    0
                } else if p.is_dir() {
                    if depth < 4 && !filer_intake::intake::in_combined_dir(&p) {
                        walk(&p, depth + 1)
                    } else {
                        0
                    }
                } else {
                    usize::from(
                        filer_intake::intake::kind_of(&p)
                            != filer_intake::intake::Kind::Unsupported,
                    )
                }
            })
            .sum()
    }
    inboxes.iter().map(|i| walk(i, 0)).sum()
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

    /// The Intake tab badge: files waiting to be processed or filed (UI_SPEC: silent, a count
    /// only).
    pub fn badge(&self) -> usize {
        self.waiting() + self.in_inboxes
    }

    /// Keeps the worker pointed at this PC's folders and collects what it did. Every frame,
    /// whatever tab is showing.
    pub fn tick(&mut self, ctx: &egui::Context, root: &Path, device_id: &str, prefs: &Ingest) {
        self.poll_jobs();
        if let Some(w) = &self.worker {
            for ev in w.events() {
                let line = match ev {
                    Event::Processed(p) => {
                        self.delivered += p.delivered.len();
                        let staging = p.staged.parent().map(Path::to_path_buf).unwrap_or_default();
                        self.to_check.extend(p.delivered.iter().map(|d| {
                            (
                                d.path.clone(),
                                staging.clone(),
                                [d.sha256.clone(), p.original_sha256.clone()],
                            )
                        }));
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
                self.log_line(line);
            }
        }
        if self.checked.is_some_and(|t| t.elapsed() < RECHECK) {
            return;
        }
        self.checked = Some(Instant::now());
        let inboxes = filer_inboxes(root);
        self.inbox = inboxes.first().cloned();
        self.in_inboxes = count_queued(&inboxes);
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
            tesseract: self.tesseract.clone(),
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

    /// Checks newly delivered files against the catalog (every frame, cheap when nothing
    /// arrived) and applies a choice made on one.
    pub fn check_dupes(&mut self, conn: &vf_core::db::Connection) {
        if let Some((i, remove)) = self.pre_action.take()
            && i < self.pre_dupes.len()
        {
            let d = self.pre_dupes.remove(i);
            if remove {
                let name = file_name(&d.path);
                let line = match vf_ingest::pipeline::move_aside(
                    &d.path,
                    &d.staging,
                    vf_ingest::pipeline::DUPLICATES_DIR,
                ) {
                    Ok(_) => format!("{name} taken out of Filer's inbox (kept in _duplicates)."),
                    Err(e) => format!("{name}: {e}"),
                };
                self.log_line(line);
            }
        }
        // Hashes come from the pipeline thread: only catalog lookups here.
        for (path, staging, hashes) in std::mem::take(&mut self.to_check) {
            let found = hashes.iter().find_map(|h| vf_fs::dupes::existing(conn, h));
            if let Some(existing) = found {
                self.pre_dupes.push(PreDup {
                    path,
                    staging,
                    existing,
                });
            }
        }
    }

    fn log_line(&mut self, line: String) {
        self.log.insert(0, line);
        self.log.truncate(LOG_MAX);
    }

    fn poll_jobs(&mut self) {
        if let Some(rx) = &self.devices_job {
            match rx.try_recv() {
                Ok(d) => {
                    self.devices = Some(d);
                    self.devices_job = None;
                }
                Err(TryRecvError::Disconnected) => self.devices_job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.scan_job {
            let line = match rx.try_recv() {
                Ok(Ok((pdf, a))) => Some(format!("Scanned {} ({})", file_name(&pdf), a.note())),
                Ok(Err(e)) => Some(format!("Scan failed: {e}")),
                Err(TryRecvError::Disconnected) => Some("Scan stopped.".into()),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(l) = line {
                self.scan_job = None;
                self.log_line(l);
            }
        }
    }

    /// Lists the scanners on a thread (TWAIN and WIA enumeration can take a moment).
    fn list_devices(&mut self, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scanner::devices());
            ctx.request_repaint();
        });
        self.devices = None;
        self.devices_job = Some(rx);
    }

    pub fn scanning(&self) -> bool {
        self.scan_job.is_some()
    }

    /// Scan row: scanner, profile, Scan. The PDF lands in the Scans folder; the worker splits
    /// and delivers it like any other file there.
    fn scan_row(&mut self, ui: &mut egui::Ui, prefs: &Ingest) {
        if self.devices.is_none() && self.devices_job.is_none() {
            self.list_devices(ui.ctx());
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Scanner").strong());
            let Some(found) = self.devices.clone() else {
                ui.spinner();
                return;
            };
            let devices = found.list;
            let user = crate::config::windows_user();
            let Some(current) = scanner::pick(&devices, prefs.scanner_for(&user)).cloned() else {
                ui.label(widgets::caption(
                    "No scanner on this PC (a TWAIN driver such as PaperStream IP, or Windows › Settings › Printers & scanners). Scans saved into the Scans folder are still processed.",
                ));
                if ui.small_button("Look again").clicked() {
                    self.list_devices(ui.ctx());
                }
                for n in &found.notes {
                    ui.label(widgets::caption(n));
                }
                return;
            };
            let mut p = prefs.clone();
            egui::ComboBox::from_id_salt("vf-scanner")
                .selected_text(&current.name)
                .show_ui(ui, |ui| {
                    for d in &devices {
                        if ui.selectable_label(d.id == current.id, &d.name).clicked() {
                            p.choose_scanner(&user, &d.id);
                        }
                    }
                });
            if ui
                .small_button("Look again")
                .on_hover_text("List the scanners again (after plugging one in or reconnecting a session).")
                .clicked()
            {
                self.list_devices(ui.ctx());
            }
            if current.kind == Kind::Twain {
                ui.checkbox(&mut p.driver_ui, "Use the driver's own window")
                    .on_hover_text("The scanner driver's window opens and its settings and profiles (PaperStream IP) are used instead of the profile here.");
            }
            let profiles = prefs.profiles();
            let profile = prefs
                .scan_profile
                .as_ref()
                .and_then(|n| profiles.iter().find(|x| &x.name == n))
                .unwrap_or(&profiles[0])
                .clone();
            let profile_box = egui::ComboBox::from_id_salt("vf-scan-profile")
                .selected_text(&profile.name);
            ui.add_enabled_ui(!(current.kind == Kind::Twain && prefs.driver_ui), |ui| {
                profile_box
                .show_ui(ui, |ui| {
                    for x in &profiles {
                        if ui.selectable_label(x.name == profile.name, &x.name).clicked() {
                            p.scan_profile = Some(x.name.clone());
                        }
                    }
                })
            });
            if p != *prefs {
                self.changed = Some(p);
            }
            let scans = prefs.scans.clone();
            let busy = self.scanning();
            let go = ui
                .add_enabled(scans.is_some() && !busy, egui::Button::new("Scan"))
                .on_disabled_hover_text(if busy {
                    "Scanning…"
                } else {
                    "Choose a Scans folder first."
                });
            if busy {
                ui.spinner();
            }
            if go.clicked()
                && let Some(dir) = scans
            {
                let (tx, rx) = std::sync::mpsc::channel();
                let (ctx, id) = (ui.ctx().clone(), current.id.clone());
                let driver_ui = current.kind == Kind::Twain && prefs.driver_ui;
                std::thread::spawn(move || {
                    let _ = tx.send(scanner::scan_to(&id, &profile, driver_ui, &dir));
                    ctx.request_repaint();
                });
                self.scan_job = Some(rx);
            }
            for n in &found.notes {
                ui.label(widgets::caption(n));
            }
        });
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
                self.scan_row(ui, prefs);
                for (i, d) in self.pre_dupes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} is already in the cabinet as {}.",
                                file_name(&d.path),
                                d.existing
                            ))
                            .color(theme::WARN),
                        );
                        if ui.button("Remove from inbox").clicked() {
                            self.pre_action = Some((i, true));
                        }
                        if ui.button("Keep it").clicked() {
                            self.pre_action = Some((i, false));
                        }
                    });
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_what_filer_would_queue() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir_all(a.join("_combined")).unwrap();
        for f in [
            "x.pdf",
            "y.zip",
            "notes.txt",
            ".hidden.pdf",
            "sub/z.PDF",
            "_combined/c.pdf",
        ] {
            std::fs::write(a.join(f), b"x").unwrap();
        }
        assert_eq!(count_queued(&[a, t.path().join("missing")]), 3);
    }
}
