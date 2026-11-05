//! The vibe-move window: the two folders, the client list, the three steps, preview, confirm,
//! progress and the report. Scans, previews and moves run on their own threads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{RichText, Ui};
use vf_move::{Client, Options, Plan, Report, Rule};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    #[default]
    Permanent,
    Date,
    Year,
}

impl Step {
    fn title(self) -> &'static str {
        match self {
            Step::Permanent => "1  Permanent folders",
            Step::Date => "2  Recently modified",
            Step::Year => "3  Year in the name",
        }
    }
}

/// Work on a thread, with its result coming back.
struct Job<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    fn spawn(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Job<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        Job { rx }
    }

    /// `Some` once, when the work is done.
    fn take(&self) -> Option<Option<T>> {
        match self.rx.try_recv() {
            Ok(v) => Some(Some(v)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(None),
        }
    }
}

#[derive(Default)]
pub struct MoveApp {
    pub old: String,
    pub new: String,
    /// The old folder's clients and whether each is ticked.
    pub clients: Vec<(Client, bool)>,
    scanning: Option<Job<Result<Vec<Client>, String>>>,
    scanned: Arc<AtomicUsize>,
    pub step: Step,
    pub permanent: String,
    pub date: String,
    pub year: String,
    /// Clients per run (owner 2026-11-05: a few at a time to try it); blank = all.
    pub per_run: String,
    /// The preview of the step: the rule and what it moves.
    pub preview: Option<(Rule, Plan)>,
    /// The folders the preview was made for: a move only ever runs on those.
    previewed: Option<(PathBuf, PathBuf)>,
    planning: Option<Job<(Rule, Plan)>>,
    pub confirm: bool,
    running: Option<Job<Result<Report, String>>>,
    progress: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    /// The last run's report.
    pub report: Option<Report>,
    /// The last run was an undo (the report says "put back").
    pub report_undo: bool,
    /// Asking "put back N files from this log?": (log, files).
    pub undo_ask: Option<(PathBuf, usize)>,
    /// Files the running job has to do (for the progress bar).
    total: usize,
    pub error: Option<String>,
    /// Tests: copy, check and delete even on one drive.
    pub force_copy: bool,
}

fn size(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

impl MoveApp {
    pub fn new() -> MoveApp {
        MoveApp {
            permanent: vf_move::PERMANENT.into(),
            ..Default::default()
        }
    }

    pub fn busy(&self) -> bool {
        self.scanning.is_some() || self.planning.is_some() || self.running.is_some()
    }

    fn old_path(&self) -> PathBuf {
        PathBuf::from(self.old.trim())
    }

    fn new_path(&self) -> PathBuf {
        PathBuf::from(self.new.trim())
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let old = self.old_path();
        if !old.is_dir() {
            self.error = Some("Choose the old folder first.".into());
            return;
        }
        self.error = None;
        self.preview = None;
        self.scanned.store(0, Ordering::Relaxed);
        let done = self.scanned.clone();
        self.scanning = Some(Job::spawn(ctx, move || {
            vf_move::scan(&old, &AtomicBool::new(false), &done)
        }));
    }

    /// The rule the step's fields say, or why not.
    pub fn rule(&self) -> Result<Rule, String> {
        match self.step {
            Step::Permanent => {
                let n = self.permanent.trim();
                if n.is_empty() {
                    return Err("Type the permanent folder's name.".into());
                }
                if n.eq_ignore_ascii_case("_Vibe") {
                    return Err("_Vibe folders never move.".into());
                }
                Ok(Rule::Permanent(n.into()))
            }
            Step::Date => vf_move::parse_date(&self.date)
                .map(Rule::ModifiedSince)
                .ok_or_else(|| "Type a date like 2024-01-31 or 1/31/2024.".into()),
            Step::Year => match self.year.trim().parse::<i32>() {
                Ok(y) if (vf_move::FIRST_YEAR..=2100).contains(&y) => Ok(Rule::YearFrom(y)),
                _ => Err(format!(
                    "Type a four-digit year from {}.",
                    vf_move::FIRST_YEAR
                )),
            },
        }
    }

    fn start_preview(&mut self, ctx: &egui::Context) {
        let rule = match self.rule() {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Err(e) = vf_move::check_folders(&self.old_path(), &self.new_path()) {
            self.error = Some(e);
            return;
        }
        self.error = None;
        self.report = None;
        // A new preview replaces the old one at once: nothing can be moved from a stale plan.
        self.preview = None;
        self.confirm = false;
        self.previewed = Some((self.old_path(), self.new_path()));
        let max = match self.per_run.trim() {
            "" => None,
            t => match t.parse::<usize>() {
                Ok(n) if n > 0 => Some(n),
                _ => {
                    self.error = Some("Clients per run: a number, or blank for all.".into());
                    return;
                }
            },
        };
        let old = self.old_path();
        let ticked: Vec<String> = self
            .clients
            .iter()
            .filter(|c| c.1)
            .map(|c| c.0.name.clone())
            .collect();
        self.planning = Some(Job::spawn(ctx, move || {
            let p = vf_move::plan_some(&old, &ticked, &rule, max);
            (rule, p)
        }));
    }

    fn start_move(&mut self, ctx: &egui::Context) {
        let Some((rule, plan)) = self.preview.clone() else {
            return;
        };
        self.confirm = false;
        // Only on the folders previewed (they may have been edited since).
        if self.previewed.as_ref() != Some(&(self.old_path(), self.new_path())) {
            self.preview = None;
            self.error = Some("The folders changed since the preview: preview again.".into());
            return;
        }
        self.report_undo = false;
        self.total = plan.items.len();
        self.progress.store(0, Ordering::Relaxed);
        self.stop.store(false, Ordering::Relaxed);
        let (old, new) = (self.old_path(), self.new_path());
        let (progress, stop) = (self.progress.clone(), self.stop.clone());
        let opts = Options {
            force_copy: self.force_copy,
            ..Default::default()
        };
        self.running = Some(Job::spawn(ctx, move || {
            vf_move::apply(&old, &new, &plan, &rule, &opts, &progress, &stop)
        }));
    }

    /// Asks before putting back what `log` moved.
    pub fn ask_undo(&mut self, log: PathBuf) {
        match vf_move::read_log(&log) {
            Ok(rows) if rows.is_empty() => {
                self.error = Some("That log lists no moved files.".into());
            }
            Ok(rows) => {
                self.error = None;
                self.undo_ask = Some((log, rows.len()));
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn start_undo(&mut self, ctx: &egui::Context) {
        let Some((log, n)) = self.undo_ask.take() else {
            return;
        };
        self.report_undo = true;
        self.total = n;
        self.preview = None;
        self.progress.store(0, Ordering::Relaxed);
        self.stop.store(false, Ordering::Relaxed);
        let (progress, stop) = (self.progress.clone(), self.stop.clone());
        let opts = Options {
            force_copy: self.force_copy,
            ..Default::default()
        };
        self.running = Some(Job::spawn(ctx, move || {
            vf_move::undo(&log, &opts, &progress, &stop)
        }));
    }

    /// Picks up finished work.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(j) = &self.scanning
            && let Some(r) = j.take()
        {
            self.scanning = None;
            match r.unwrap_or_else(|| Err("the scan stopped".into())) {
                Ok(list) => {
                    // Ticks kept for clients still there; new ones ticked.
                    let was: std::collections::HashMap<String, bool> = self
                        .clients
                        .iter()
                        .map(|(c, t)| (c.name.clone(), *t))
                        .collect();
                    self.clients = list
                        .into_iter()
                        .map(|c| {
                            let t = was.get(&c.name).copied().unwrap_or(true);
                            (c, t)
                        })
                        .collect();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(j) = &self.planning
            && let Some(r) = j.take()
        {
            self.planning = None;
            match r {
                Some(p) => self.preview = Some(p),
                None => self.error = Some("the preview stopped".into()),
            }
        }
        if let Some(j) = &self.running
            && let Some(r) = j.take()
        {
            self.running = None;
            self.preview = None;
            match r.unwrap_or_else(|| Err("the move stopped".into())) {
                Ok(rep) => self.report = Some(rep),
                Err(e) => self.error = Some(e),
            }
            // The counts changed.
            self.start_scan(ctx);
        }
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let busy = self.busy();
        ui.heading("Vibe Move");
        ui.label(
            "Moves files from an over-large client tree into a new folder, keeping their paths: \
             the old folder becomes the archive, the new one your active cabinet. Files are moved, \
             not copied; every move is written to a log, and a move can be undone from it.",
        );
        ui.add_space(6.0);
        let mut changed: Vec<&str> = vec![];
        egui::Grid::new("folders").num_columns(3).show(ui, |ui| {
            for (label, field, hint) in [
                ("Old folder (the archive)", &mut self.old, r"T:\Clients"),
                (
                    "New folder (active cabinet)",
                    &mut self.new,
                    r"T:\Clients Active",
                ),
            ] {
                let l = ui.label(label);
                let before = field.clone();
                ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(field)
                        .hint_text(hint)
                        .desired_width(420.0),
                )
                .labelled_by(l.id);
                if ui
                    .add_enabled(!busy, egui::Button::new(format!("Choose… ({label})")))
                    .clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_folder()
                {
                    *field = p.display().to_string();
                }
                if *field != before {
                    changed.push(label);
                }
                ui.end_row();
            }
        });
        // Another folder: the preview (and, for the old folder, its client list) is gone.
        if !changed.is_empty() {
            self.preview = None;
            self.confirm = false;
            if changed.iter().any(|l| l.starts_with("Old")) {
                self.clients.clear();
            }
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Scan the old folder"))
                .clicked()
            {
                self.start_scan(&ctx);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Undo a move from its log…"))
                .on_hover_text("Pick the CSV log a move wrote; the files it moved go back.")
                .clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("Move log", &["csv"])
                    .pick_file()
            {
                self.ask_undo(p);
            }
            if self.scanning.is_some() {
                ui.spinner();
                ui.label(format!(
                    "Counting files: {} client folders so far…",
                    self.scanned.load(Ordering::Relaxed)
                ));
            }
        });
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(egui::Color32::from_rgb(0xb4, 0x23, 0x18)));
        }
        ui.separator();
        ui.columns(2, |cols| {
            self.client_list(&mut cols[0], busy);
            self.step_panel(&mut cols[1], &ctx, busy);
        });
    }

    fn client_list(&mut self, ui: &mut Ui, busy: bool) {
        let ticked = self.clients.iter().filter(|c| c.1).count();
        ui.label(
            RichText::new(format!(
                "Clients ({ticked} of {} ticked)",
                self.clients.len()
            ))
            .strong(),
        );
        if self.clients.is_empty() {
            ui.label("Scan the old folder to list its client folders.");
            return;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Tick all"))
                .clicked()
            {
                self.clients.iter_mut().for_each(|c| c.1 = true);
                self.preview = None;
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Untick all"))
                .clicked()
            {
                self.clients.iter_mut().for_each(|c| c.1 = false);
                self.preview = None;
            }
        });
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("clients")
            .max_height(ui.available_height() - 8.0)
            .show_rows(ui, 22.0, self.clients.len(), |ui, range| {
                for i in range {
                    let (c, on) = &mut self.clients[i];
                    let name = if c.name.is_empty() {
                        "(files directly in the old folder)".to_string()
                    } else {
                        c.name.clone()
                    };
                    changed |= ui
                        .add_enabled(
                            !busy,
                            egui::Checkbox::new(
                                on,
                                format!("{name}   {} files, {}", c.files, size(c.bytes)),
                            ),
                        )
                        .changed();
                }
            });
        if changed {
            self.preview = None;
        }
    }

    fn step_panel(&mut self, ui: &mut Ui, ctx: &egui::Context, busy: bool) {
        ui.horizontal(|ui| {
            for s in [Step::Permanent, Step::Date, Step::Year] {
                if ui
                    .add_enabled(!busy, egui::Button::selectable(self.step == s, s.title()))
                    .clicked()
                    && self.step != s
                {
                    self.step = s;
                    self.preview = None;
                }
            }
        });
        ui.add_space(4.0);
        let (label, field, hint, help) = match self.step {
            Step::Permanent => (
                "Permanent folder name",
                &mut self.permanent,
                "Permanent",
                "Each ticked client's top-level folder with this name (any letter case) moves whole, with its sub-folders.",
            ),
            Step::Date => (
                "Modified on or after",
                &mut self.date,
                "2024-01-31",
                "Every file in the ticked clients whose Date modified is on or after this date.",
            ),
            Step::Year => (
                "Year or later",
                &mut self.year,
                "2023",
                "Files with a four-digit year at or after this one in the file name or in a folder inside the client folder (Taxes\\2024\\…). With several years, the newest counts. Files with no year stay.",
            ),
        };
        ui.horizontal(|ui| {
            let l = ui.label(label);
            if ui
                .add_enabled(
                    !busy,
                    egui::TextEdit::singleline(field)
                        .hint_text(hint)
                        .desired_width(140.0),
                )
                .labelled_by(l.id)
                .changed()
            {
                self.preview = None;
            }
        });
        ui.label(RichText::new(help).small());
        ui.horizontal(|ui| {
            let l = ui.label("Clients per run");
            if ui
                .add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut self.per_run)
                        .hint_text("all")
                        .desired_width(60.0),
                )
                .labelled_by(l.id)
                .on_hover_text(
                    "Move only this many clients' files this time (counting only clients that have files for the step); blank moves them all. Preview again after a move for the next ones.",
                )
                .changed()
            {
                self.preview = None;
            }
        });
        ui.horizontal(|ui| {
            let can = !busy && self.clients.iter().any(|c| c.1);
            if ui.add_enabled(can, egui::Button::new("Preview")).clicked() {
                self.start_preview(ctx);
            }
            if self.planning.is_some() {
                ui.spinner();
                ui.label("Finding the files…");
            }
        });
        ui.separator();
        if let Some((log, n)) = self.undo_ask.clone() {
            ui.label(format!(
                "Put back the {n} files moved in {}? Each goes back to where it was; a file changed since the move is left where it is.",
                log.display()
            ));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.busy(),
                        egui::Button::new(format!("Yes, put back {n} files")),
                    )
                    .clicked()
                {
                    self.start_undo(ctx);
                }
                if ui.button("Cancel").clicked() {
                    self.undo_ask = None;
                }
            });
            ui.separator();
            return;
        }
        if self.running.is_some() {
            let total = self.total.max(1);
            let done = self.progress.load(Ordering::Relaxed);
            let what = if self.report_undo {
                "Putting back"
            } else {
                "Moving"
            };
            ui.add(
                egui::ProgressBar::new(done as f32 / total as f32)
                    .text(format!("{what}: {done} of {total} files")),
            );
            if ui.button("Stop after this file").clicked() {
                self.stop.store(true, Ordering::Relaxed);
            }
            return;
        }
        if let Some(rep) = self.report.clone() {
            report(ui, &rep, self.report_undo);
            if !self.report_undo
                && rep.moved > 0
                && let Some(log) = &rep.log
                && ui.button("Undo this move").clicked()
            {
                self.ask_undo(log.clone());
            }
            ui.separator();
        }
        let Some((rule, plan)) = self.preview.clone() else {
            return;
        };
        let n = plan.items.len();
        ui.label(
            RichText::new(format!(
                "{n} files ({}) from {} clients would move: {}",
                size(plan.bytes()),
                plan.clients,
                rule.label()
            ))
            .strong(),
        );
        if plan.more {
            ui.label(
                "More ticked clients have files for this step: after this move, Preview again for the next ones.",
            );
        }
        if plan.clutter > 0 {
            ui.label(format!(
                "{} Windows files (Thumbs.db, desktop.ini, ~$ and .tmp files) stay where they are.",
                plan.clutter
            ));
        }
        for u in plan.unreadable.iter().take(5) {
            ui.label(format!("Could not be read: {u}"));
        }
        if n == 0 {
            return;
        }
        if self.confirm {
            ui.label(format!(
                "Move {n} files ({}) from {} to {}? They are moved, not copied. A log of every move is kept; Undo puts them back from it.",
                size(plan.bytes()),
                self.old.trim(),
                self.new.trim()
            ));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.busy(),
                        egui::Button::new(format!("Yes, move {n} files")),
                    )
                    .clicked()
                {
                    self.start_move(ctx);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm = false;
                }
            });
        } else if ui.button(format!("Move {n} files")).clicked() {
            self.confirm = true;
        }
        let items = plan.items;
        egui::ScrollArea::vertical().id_salt("preview").show_rows(
            ui,
            20.0,
            items.len(),
            |ui, range| {
                for it in &items[range] {
                    ui.label(format!(
                        "{}   ({}, {})",
                        it.rel.display(),
                        it.why,
                        size(it.size)
                    ));
                }
            },
        );
    }
}

fn report(ui: &mut Ui, rep: &Report, undo: bool) {
    ui.label(
        RichText::new(format!(
            "{} {} files ({}){}; {} folders left empty were removed.",
            match (rep.stopped, undo) {
                (true, false) => "Stopped after moving",
                (false, false) => "Moved",
                (true, true) => "Stopped after putting back",
                (false, true) => "Put back",
            },
            rep.moved,
            size(rep.bytes),
            if rep.kept_both > 0 {
                format!(
                    ", {} under a new name because the name was taken",
                    rep.kept_both
                )
            } else {
                String::new()
            },
            rep.folders_removed
        ))
        .strong(),
    );
    if let Some(e) = &rep.log_error {
        ui.label(
            RichText::new(format!("Stopped: {e}")).color(egui::Color32::from_rgb(0xb4, 0x23, 0x18)),
        );
    }
    if !rep.failed.is_empty() {
        ui.label(format!(
            "{} files were not {}:",
            rep.failed.len(),
            if undo { "put back" } else { "moved" }
        ));
        for (p, why) in rep.failed.iter().take(20) {
            ui.label(format!("{}: {why}", p.display()));
        }
    }
    if let Some(log) = &rep.log {
        ui.horizontal(|ui| {
            ui.label(format!("Log: {}", log.display()));
            if ui.button("Open the log").clicked() {
                open(log);
            }
        });
    }
}

fn open(p: &Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn put(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn wait(h: &mut Harness<'_, MoveApp>) {
        let t = std::time::Instant::now();
        while h.state().busy() {
            assert!(t.elapsed() < std::time::Duration::from_secs(20), "stuck");
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.step();
        }
        h.run_steps(3);
    }

    fn type_into(h: &mut Harness<'_, MoveApp>, label: &str, text: &str) {
        h.get_by_label(label).click();
        h.step();
        h.get_by_label(label).type_text(text);
        h.run_steps(2);
    }

    fn click(h: &mut Harness<'_, MoveApp>, label: &str) {
        h.get_by_role_and_label(Role::Button, label).click();
        h.run_steps(2);
    }

    /// PA-35 done-when: the window drives a three-step run on a fixture tree.
    #[test]
    fn three_steps_in_the_window() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Old");
        let new = t.path().join("New");
        put(&old.join("Sample, Pat/Permanent/EIN.pdf"), "ein");
        put(&old.join("Sample, Pat/Taxes/2018/W2.pdf"), "old");
        put(&old.join("Sample, Pat/Taxes/2024/W2.pdf"), "new");
        put(&old.join("Example Co/Bank 2025-03.pdf"), "bank");
        let mut app = MoveApp::new();
        app.old = old.display().to_string();
        app.new = new.display().to_string();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1100.0, 800.0))
            .build_ui_state(|ui, s: &mut MoveApp| s.ui(ui), app);
        click(&mut h, "Scan the old folder");
        wait(&mut h);
        assert_eq!(h.state().clients.len(), 2);

        // 1: Permanent.
        click(&mut h, "Preview");
        wait(&mut h);
        assert_eq!(h.state().preview.as_ref().unwrap().1.items.len(), 1);
        click(&mut h, "Move 1 files");
        click(&mut h, "Yes, move 1 files");
        wait(&mut h);
        assert!(new.join("Sample, Pat/Permanent/EIN.pdf").is_file());
        assert!(
            !old.join("Sample, Pat/Permanent").exists(),
            "emptied folder removed"
        );
        assert_eq!(h.state().report.as_ref().unwrap().moved, 1);

        // 3: Year 2024 or later (the date step works the same way; its rule is unit-tested).
        click(&mut h, "3  Year in the name");
        type_into(&mut h, "Year or later", "2024");
        click(&mut h, "Preview");
        wait(&mut h);
        assert_eq!(h.state().preview.as_ref().unwrap().1.items.len(), 2);
        click(&mut h, "Move 2 files");
        click(&mut h, "Yes, move 2 files");
        wait(&mut h);
        assert!(new.join("Sample, Pat/Taxes/2024/W2.pdf").is_file());
        assert!(new.join("Example Co/Bank 2025-03.pdf").is_file());
        assert!(
            old.join("Sample, Pat/Taxes/2018/W2.pdf").is_file(),
            "2018 stays"
        );

        // Undo it: both back where they were.
        click(&mut h, "Undo this move");
        click(&mut h, "Yes, put back 2 files");
        wait(&mut h);
        assert!(old.join("Sample, Pat/Taxes/2024/W2.pdf").is_file());
        assert!(old.join("Example Co/Bank 2025-03.pdf").is_file());
        assert!(!new.join("Example Co").exists());
        assert_eq!(h.state().report.as_ref().unwrap().moved, 2);
        assert!(h.state().report_undo);

        // 2: a date in the future moves nothing.
        click(&mut h, "2  Recently modified");
        type_into(&mut h, "Modified on or after", "2099-01-01");
        click(&mut h, "Preview");
        wait(&mut h);
        assert_eq!(h.state().preview.as_ref().unwrap().1.items.len(), 0);
        // A bad date is explained.
        type_into(&mut h, "Modified on or after", "x");
        click(&mut h, "Preview");
        h.run_steps(2);
        assert!(h.state().error.as_deref().unwrap_or("").contains("date"));
    }
}

#[cfg(test)]
mod review_tests {
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Review fix: a folder edited after the preview drops it, so Move never runs a plan made
    /// for other folders.
    #[test]
    fn editing_a_folder_drops_the_preview() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Old");
        std::fs::create_dir_all(old.join("Sample, Pat/Permanent")).unwrap();
        std::fs::write(old.join("Sample, Pat/Permanent/EIN.pdf"), "ein").unwrap();
        let mut app = MoveApp::new();
        app.old = old.display().to_string();
        app.new = t.path().join("New").display().to_string();
        let mut h = Harness::builder()
            .with_size(egui::vec2(1100.0, 800.0))
            .build_ui_state(|ui, s: &mut MoveApp| s.ui(ui), app);
        let settle = |h: &mut Harness<'_, MoveApp>| {
            let t0 = std::time::Instant::now();
            while h.state().busy() {
                assert!(t0.elapsed() < std::time::Duration::from_secs(20));
                std::thread::sleep(std::time::Duration::from_millis(20));
                h.step();
            }
            h.run_steps(3);
        };
        h.get_by_role_and_label(Role::Button, "Scan the old folder")
            .click();
        h.run_steps(2);
        settle(&mut h);
        h.get_by_role_and_label(Role::Button, "Preview").click();
        h.run_steps(2);
        settle(&mut h);
        assert!(h.state().preview.is_some());
        h.get_by_label("New folder (active cabinet)").click();
        h.step();
        h.get_by_label("New folder (active cabinet)").type_text("2");
        h.run_steps(3);
        assert!(h.state().preview.is_none(), "the preview is gone");
        assert!(h.query_by_label("Move 1 files").is_none());
        // And a move started from a stale state is refused.
        let s = h.state_mut();
        s.preview = Some((
            vf_move::Rule::Permanent("Permanent".into()),
            vf_move::Plan::default(),
        ));
        s.new = t.path().join("Other").display().to_string();
        let ctx = egui::Context::default();
        s.start_move(&ctx);
        assert!(s.error.as_deref().unwrap_or("").contains("preview again"));
    }
}
