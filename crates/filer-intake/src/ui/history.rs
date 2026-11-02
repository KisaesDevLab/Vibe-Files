//! History tab (UI_SPEC §13): last-batch card with the undo check and Undo, filters, and the
//! operation log newest first, grouped by batch.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use uuid::Uuid;

use crate::app::AppState;
use crate::clients::ClientList;
use crate::commit::{LastBatch, Method};
use crate::config::Level;
use crate::oplog::{self, Op, OpRecord, OpResult, ReadResult};
use crate::ui::setup::banner;
use crate::ui::theme::{self, PillKind, card, label_caps, mono, pal, pill, primary_button, strong};
use crate::ui::widgets::show_in_explorer;
use crate::undo::{self, UndoReport};
use crate::worker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ResultFilter {
    #[default]
    All,
    Ok,
    Errors,
}

#[derive(Default)]
pub struct HistoryScreen {
    /// Newest first.
    records: Vec<OpRecord>,
    bad_lines: usize,
    loaded_generation: Option<u64>,
    load_job: Option<Receiver<Result<ReadResult, String>>>,
    client: String,
    from: String,
    to: String,
    batch: Option<Uuid>,
    result: ResultFilter,
    op: Option<Op>,
    undo_confirm: bool,
    undo_job: Option<Receiver<UndoReport>>,
    message: Option<(Level, String)>,
    /// Copies of the last batch changed since they were written (hashed on a worker), by batch.
    modified: Option<(Uuid, Vec<PathBuf>)>,
    modified_job: Option<Receiver<(Uuid, Vec<PathBuf>)>>,
}

impl HistoryScreen {
    #[cfg(test)]
    pub fn loading(&self) -> bool {
        self.load_job.is_some() || self.loaded_generation.is_none()
    }

    fn reload_if_needed(&mut self, ctx: &egui::Context, state: &AppState) {
        if let Some(rx) = &self.load_job
            && let Ok(result) = rx.try_recv()
        {
            self.load_job = None;
            match result {
                Ok(r) => {
                    self.bad_lines = r.bad_lines.len();
                    self.records = r.records;
                    self.records.reverse();
                }
                Err(e) => {
                    self.message = Some((Level::Error, format!("Cannot read ops.jsonl: {e}")))
                }
            }
        }
        if self.load_job.is_none() && self.loaded_generation != Some(state.ops_generation) {
            self.loaded_generation = Some(state.ops_generation);
            let path = oplog::path(&state.data_dir);
            self.load_job = Some(worker::background(ctx, move || {
                oplog::read(&path).map_err(|e| e.to_string())
            }));
        }
    }

    fn poll_undo(&mut self, ctx: &egui::Context, state: &mut AppState) {
        let Some(rx) = &self.undo_job else { return };
        let Ok(report) = rx.try_recv() else { return };
        self.undo_job = None;
        let back = report.outcomes.iter().filter(|o| o.1.is_ok()).count();
        let failed: Vec<String> = report
            .outcomes
            .iter()
            .filter_map(|(p, r)| r.as_ref().err().map(|e| format!("{}: {e}", p.display())))
            .chain(report.warnings.iter().cloned())
            .collect();
        let sentinels_removed = !report.last_batch.created_files.is_empty();
        // Renamed client folders went back to their old names (V9): the index follows.
        let moves: Vec<_> = report
            .last_batch
            .renamed_dirs
            .iter()
            .filter(|r| r.from.is_dir())
            .map(|r| (r.to.clone(), r.from.clone()))
            .collect();
        if !moves.is_empty() {
            let data_dir = state.data_dir.clone();
            state.folders.follow_renames(&data_dir, &moves);
        }
        state.last_batch = Some(report.last_batch);
        state.ops_generation += 1;
        if sentinels_removed {
            state.start_scan(ctx);
        }
        self.message = Some(if failed.is_empty() {
            (
                Level::Info,
                format!("Undone: {back} file{} back.", plural(back)),
            )
        } else {
            (
                Level::Error,
                format!("Undone with problems: {back} back. {}", failed.join(" | ")),
            )
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let ctx = ui.ctx().clone();
        if let Some(b) = state.history_filter.take() {
            self.batch = Some(b);
        }
        self.reload_if_needed(&ctx, state);
        self.poll_undo(&ctx, state);
        self.check_copies(&ctx, state);

        ui.add_space(4.0);
        self.undo_ui(ui, state);
        if let Some((level, msg)) = &self.message {
            banner(ui, *level, msg);
        }
        ui.add_space(8.0);
        let clients = state.clients.list().cloned();
        let shown = self.visible(clients.as_deref()).len();
        self.filters_ui(ui, shown);
        if self.bad_lines > 0 {
            banner(
                ui,
                Level::Error,
                &format!(
                    "{} line(s) of ops.jsonl could not be read and are not shown.",
                    self.bad_lines
                ),
            );
        }
        ui.add_space(6.0);
        let rows = self.visible(clients.as_deref());
        if let Some(batch) = self.table_ui(ui, &rows, state) {
            state.import_show = Some(batch);
            state.goto = Some(crate::app::Tab::Import);
        }
        self.confirm_ui(&ctx, state);
    }

    /// Hashes the last batch's copies on a worker when the batch changes (undo preflight).
    fn check_copies(&mut self, ctx: &egui::Context, state: &AppState) {
        if let Some(rx) = &self.modified_job
            && let Ok(result) = rx.try_recv()
        {
            self.modified = Some(result);
            self.modified_job = None;
        }
        let Some(lb) = &state.last_batch else { return };
        let has_copies = lb.items.iter().any(|i| !i.copies.is_empty());
        let current = self.modified.as_ref().is_some_and(|(b, _)| *b == lb.batch);
        if has_copies && !current && self.modified_job.is_none() && !lb.undone {
            let lb = lb.clone();
            self.modified_job = Some(worker::background(ctx, move || {
                (lb.batch, undo::modified_copies(&lb))
            }));
        }
    }

    /// Records read from ops.jsonl (footer).
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Ctrl+Z: open the Undo confirmation, or say why it isn't possible.
    pub fn request_undo(&mut self, state: &AppState) {
        if self.undo_job.is_some() {
            return;
        }
        match undo_blocker(state) {
            None => self.undo_confirm = true,
            Some(why) => self.message = Some((Level::Error, why)),
        }
    }

    #[cfg(test)]
    pub fn undo_prompt_open(&self) -> bool {
        self.undo_confirm
    }

    fn undo_ui(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let p = pal(ui);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(ui.available_width() * 0.55);
                    label_caps(ui, "Last batch");
                    match &state.last_batch {
                        Some(lb) => {
                            ui.add(egui::Label::new(batch_line(lb)).wrap());
                        }
                        None => {
                            ui.label(RichText::new("No batch committed yet.").color(p.muted));
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let blocker = undo_blocker(state);
                    let busy = self.undo_job.is_some();
                    let text = {
                        let mut job = egui::text::LayoutJob::default();
                        let body = egui::TextStyle::Button.resolve(ui.style());
                        job.append(
                            "Undo last batch",
                            0.0,
                            egui::TextFormat::simple(body, p.text),
                        );
                        job.append(
                            "Ctrl+Z",
                            8.0,
                            egui::TextFormat::simple(egui::FontId::proportional(11.0), p.muted),
                        );
                        job
                    };
                    let button =
                        ui.add_enabled(blocker.is_none() && !busy, egui::Button::new(text));
                    button.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            blocker.is_none() && !busy,
                            "Undo last batch…",
                        )
                    });
                    let button = match &blocker {
                        Some(why) => button.on_disabled_hover_text(why),
                        None => button,
                    };
                    if button.clicked() {
                        self.undo_confirm = true;
                    }
                    if busy {
                        ui.spinner();
                        ui.label("Undoing…");
                    } else if state.last_batch.is_some() {
                        // Undo preflight: why it is or isn't available.
                        let modified = self
                            .modified
                            .as_ref()
                            .filter(|(b, _)| state.last_batch.as_ref().is_some_and(|lb| lb.batch == *b))
                            .map_or(0, |(_, m)| m.len());
                        let (text, color) = match &blocker {
                            None if modified > 0 => (
                                format!(
                                    "All primaries present at recorded size · {modified} cop{} modified since filed will be left in place and reported",
                                    if modified == 1 { "y" } else { "ies" }
                                ),
                                p.pill(PillKind::Warn).1,
                            ),
                            None => (
                                "All files are where this batch put them, at their recorded size."
                                    .to_string(),
                                p.muted,
                            ),
                            Some(why) => (why.clone(), p.pill(PillKind::Warn).1),
                        };
                        ui.add(
                            egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap(),
                        );
                    }
                });
            });
        });
    }

    fn confirm_ui(&mut self, ctx: &egui::Context, state: &AppState) {
        if !self.undo_confirm {
            return;
        }
        if undo_blocker(state).is_some() {
            self.undo_confirm = false;
            return;
        }
        let Some(lb) = state.last_batch.clone() else {
            self.undo_confirm = false;
            return;
        };
        let s = undo::summary(&lb);
        let mut confirm = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("undo_confirm")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(format!(
                "Undo batch of {}",
                lb.committed_at
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
            ));
            ui.add_space(6.0);
            ui.label(format!(
                "{} file{} go back to their inbox",
                s.files,
                plural(s.files)
            ));
            if s.restored > 0 {
                ui.label(format!(
                    "{} overwritten file{} restored",
                    s.restored,
                    plural(s.restored)
                ));
            }
            if s.copies > 0 {
                ui.label(format!(
                    "{} cop{} to related clients removed (unless changed since filed)",
                    s.copies,
                    if s.copies == 1 { "y" } else { "ies" }
                ));
            }
            if s.folders > 0 {
                ui.label(format!(
                    "{} folder{} the batch created removed if empty",
                    s.folders,
                    plural(s.folders)
                ));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(primary_button(ui, "Confirm undo")).clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if confirm {
            self.undo_confirm = false;
            self.message = None;
            let data_dir = state.data_dir.clone();
            let undo = state.hooks.undo.clone(); // VENDORED.md change 5
            self.undo_job = Some(worker::background(ctx, move || undo(&data_dir, lb, false)));
        } else if close || modal.should_close() {
            self.undo_confirm = false;
        }
    }

    fn filters_ui(&mut self, ui: &mut egui::Ui, shown: usize) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            let client = ui.add(
                egui::TextEdit::singleline(&mut self.client)
                    .hint_text("Client name or ID")
                    .desired_width(180.0),
            );
            client.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Client name or ID")
            });
            let from = ui.add(
                egui::TextEdit::singleline(&mut self.from)
                    .hint_text("YYYY-MM-DD")
                    .desired_width(100.0),
            );
            from.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "From date")
            });
            ui.label(RichText::new("to").color(p.muted));
            let to = ui.add(
                egui::TextEdit::singleline(&mut self.to)
                    .hint_text("YYYY-MM-DD")
                    .desired_width(100.0),
            );
            to.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "To date"));

            let mut batches: Vec<(Uuid, chrono::DateTime<chrono::Utc>)> = Vec::new();
            for r in &self.records {
                if !batches.iter().any(|(b, _)| *b == r.batch) {
                    batches.push((r.batch, r.ts));
                }
            }
            egui::ComboBox::from_id_salt("history_batch")
                .width(130.0)
                .selected_text(self.batch.map_or("All batches".to_string(), short_batch))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.batch, None, "All batches");
                    for (b, ts) in batches.iter().take(50) {
                        let text = format!(
                            "{} {}",
                            short_batch(*b),
                            ts.with_timezone(&chrono::Local).format("%b %-d, %-I:%M %p")
                        );
                        ui.selectable_value(&mut self.batch, Some(*b), text);
                    }
                });
            egui::ComboBox::from_id_salt("history_result")
                .width(100.0)
                .selected_text(match self.result {
                    ResultFilter::All => "Any result",
                    ResultFilter::Ok => "ok",
                    ResultFilter::Errors => "error",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.result, ResultFilter::All, "Any result");
                    ui.selectable_value(&mut self.result, ResultFilter::Ok, "ok");
                    ui.selectable_value(&mut self.result, ResultFilter::Errors, "error");
                });
            egui::ComboBox::from_id_salt("history_op")
                .width(140.0)
                .selected_text(self.op.map_or("Any op", op_name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.op, None, "Any op");
                    for op in ALL_OPS {
                        ui.selectable_value(&mut self.op, Some(op), op_name(op));
                    }
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} records · newest first",
                        theme::thousands(shown)
                    ))
                    .size(12.0)
                    .color(p.muted),
                );
            });
        });
    }

    fn visible(&self, clients: Option<&ClientList>) -> Vec<&OpRecord> {
        let day = |s: &str| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
        let (from, to) = (day(&self.from), day(&self.to));
        let query = self.client.trim().to_lowercase();
        let name_of = |id: &str| {
            clients
                .and_then(|c| c.match_by_id(id))
                .map(|c| c.name.to_lowercase())
                .unwrap_or_default()
        };
        self.records
            .iter()
            .filter(|r| {
                let local = r.ts.with_timezone(&chrono::Local).date_naive();
                (query.is_empty()
                    || r.client_id.as_deref().is_some_and(|c| {
                        c.to_lowercase().contains(&query) || name_of(c).contains(&query)
                    }))
                    && from.is_none_or(|f| local >= f)
                    && to.is_none_or(|t| local <= t)
                    && self.batch.is_none_or(|b| r.batch == b)
                    && self.op.is_none_or(|o| r.op == o)
                    && match self.result {
                        ResultFilter::All => true,
                        ResultFilter::Ok => r.result == OpResult::Ok,
                        ResultFilter::Errors => r.result == OpResult::Error,
                    }
            })
            .collect()
    }

    /// Returns the batch of a zip import whose "Show import" was clicked.
    fn table_ui(&self, ui: &mut egui::Ui, rows: &[&OpRecord], state: &AppState) -> Option<Uuid> {
        let p = pal(ui);
        if rows.is_empty() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(if self.records.is_empty() {
                        "Nothing has been filed yet."
                    } else {
                        "No entries match the filters."
                    })
                    .color(p.muted),
                );
            });
            return None;
        }
        let mut show_import = None;
        let modified: Vec<PathBuf> = self
            .modified
            .as_ref()
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let clients = state.clients.list().cloned();
        let root = state.settings.root.clone();
        let mut open: Option<PathBuf> = None;
        let width = ui.available_width();
        let widths = history_widths(width - 20.0);
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_width(width);
                Frame::new()
                    .fill(p.tint_copy)
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.horizontal(|ui| {
                            for (w, t) in widths
                                .iter()
                                .zip(["Time", "Op", "Client", "Source → Destination", "Result", ""])
                            {
                                hcell(ui, *w, |ui| {
                                    label_caps(ui, t);
                                });
                            }
                        });
                    });
                ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                egui::ScrollArea::vertical()
                    .id_salt("history_rows")
                    .auto_shrink([false, true])
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut last_batch: Option<Uuid> = None;
                        for r in rows.iter().take(MAX_ROWS) {
                            if last_batch != Some(r.batch) {
                                last_batch = Some(r.batch);
                                let n = rows.iter().filter(|x| x.batch == r.batch).count();
                                batch_header(ui, r, n, width);
                            }
                            let row = Frame::new()
                                .inner_margin(Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    ui.set_width(width - 20.0);
                                    ui.horizontal_top(|ui| {
                                        hcell(ui, widths[0], |ui| {
                                            ui.add_space(2.0);
                                            ui.label(mono(r.ts.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()).color(p.text));
                                        });
                                        hcell(ui, widths[1], |ui| {
                                            let kind = match r.op {
                                                Op::CopyVerifyDelete | Op::Copy => PillKind::Info,
                                                Op::Rollback => PillKind::Err,
                                                _ => PillKind::Neutral,
                                            };
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                pill(ui, kind, op_name(r.op));
                                                if let Some(of) = &r.copy_of {
                                                    ui.label(RichText::new(format!("of {of}")).size(11.0).color(p.muted));
                                                }
                                                if r.op == Op::Copy
                                                    && r.dst.as_ref().is_some_and(|d| modified.contains(d))
                                                {
                                                    pill(ui, PillKind::Warn, "modified since");
                                                }
                                            });
                                        });
                                        hcell(ui, widths[2], |ui| {
                                            ui.add_space(2.0);
                                            if let Some(id) = &r.client_id {
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 4.0;
                                                    if let Some(c) = clients.as_ref().and_then(|l| l.match_by_id(id)) {
                                                        ui.label(&c.name);
                                                    }
                                                    ui.label(mono(id).color(p.muted));
                                                });
                                            }
                                        });
                                        hcell(ui, widths[3], |ui| {
                                            ui.add(egui::Label::new(path_job(ui, r, root.as_deref())).wrap());
                                            // Zip entry of a routed file, or the zip import summary (P07b).
                                            if let Some(note) = &r.note {
                                                let kept = crate::commit::KEPT_BOTH;
                                                let text = note
                                                    .strip_suffix(&format!(" · {kept}"))
                                                    .or_else(|| (note == kept).then_some(""))
                                                    .unwrap_or(note);
                                                if !text.is_empty() {
                                                    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(p.muted)).wrap());
                                                }
                                                if text.len() != note.len() {
                                                    pill(ui, PillKind::Neutral, kept);
                                                }
                                            }
                                        });
                                        hcell(ui, widths[4], |ui| match r.result {
                                            OpResult::Ok => {
                                                pill(ui, PillKind::Ok, "ok");
                                            }
                                            OpResult::Error => {
                                                pill(ui, PillKind::Err, "error");
                                            }
                                        });
                                        hcell(ui, widths[5], |ui| {
                                            ui.add_space(2.0);
                                            let target = r.dst.as_ref().or(r.src.as_ref());
                                            if let Some(t) = target
                                                && ui.link("Open folder").on_hover_text("Show in Explorer").clicked()
                                            {
                                                open = Some(t.clone());
                                            }
                                            if r.op == Op::ZipImport && ui.link("Show import").clicked() {
                                                show_import = Some(r.batch);
                                            }
                                        });
                                    });
                                });
                            ui.painter().hline(
                                row.response.rect.x_range(),
                                row.response.rect.bottom(),
                                Stroke::new(1.0, p.border.gamma_multiply(0.6)),
                            );
                        }
                        if rows.len() > MAX_ROWS {
                            Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "Showing the newest {MAX_ROWS} of {} records. Use the filters to find older ones.",
                                        theme::thousands(rows.len())
                                    ))
                                    .size(12.0)
                                    .color(p.muted),
                                );
                            });
                        }
                    });
            });
        if let Some(p) = open {
            show_in_explorer(&p);
        }
        show_import
    }
}

/// Rows drawn at most; filters narrow the rest.
const MAX_ROWS: usize = 300;

const ALL_OPS: [Op; 14] = [
    Op::Move,
    Op::CopyVerifyDelete,
    Op::Mkdir,
    Op::Merge,
    Op::Split,
    Op::Undo,
    Op::Skip,
    Op::Sentinel,
    Op::Backup,
    Op::SentinelSetAside,
    Op::Copy,
    Op::Rollback,
    Op::ZipImport,
    Op::ZipEntry,
];

/// "Batch b7e1 · Today 10:42 AM · 12 records" spanning the table.
fn batch_header(ui: &mut egui::Ui, r: &OpRecord, n: usize, width: f32) {
    let p = pal(ui);
    Frame::new()
        .fill(p.tint_copy)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.label(
                strong(format!(
                    "Batch {} · {} · {n} record{}",
                    short_batch(r.batch),
                    when(r.ts),
                    plural(n)
                ))
                .size(12.0),
            );
        });
}

/// `Today 10:42 AM` or `Oct 29, 2:08 PM`.
fn when(ts: chrono::DateTime<chrono::Utc>) -> String {
    let local = ts.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() {
        format!("Today {}", local.format("%-I:%M %p"))
    } else {
        local.format("%b %-d, %-I:%M %p").to_string()
    }
}

/// "Today 10:42 AM · b7e1… · 5 moves, 1 copy-verify, 1 folder created · 6.4 MB".
fn batch_line(lb: &LastBatch) -> String {
    let moves = lb.items.iter().filter(|i| i.method == Method::Move).count();
    let copies = lb.items.len() - moves;
    let bytes: u64 = lb.items.iter().map(|i| i.bytes).sum();
    let mut parts = Vec::new();
    if moves > 0 {
        parts.push(format!("{moves} move{}", plural(moves)));
    }
    if copies > 0 {
        parts.push(format!("{copies} copy-verify"));
    }
    let related: usize = lb.items.iter().map(|i| i.copies.len()).sum();
    if related > 0 {
        let word = if related == 1 { "copy" } else { "copies" };
        parts.push(format!("{related} {word} to related clients"));
    }
    if !lb.zip_entries.is_empty() {
        let n = lb.zip_entries.len();
        parts.push(format!("{n} file{} unpacked from a zip", plural(n)));
    }
    let routed = lb.items.iter().filter(|i| i.from_zip.is_some()).count();
    if routed > 0 {
        parts.push(format!("{routed} zip routed"));
    }
    if !lb.created_dirs.is_empty() {
        let n = lb.created_dirs.len();
        parts.push(format!("{n} folder{} created", plural(n)));
    }
    if !lb.renamed_dirs.is_empty() {
        let n = lb.renamed_dirs.len();
        parts.push(format!("{n} client folder{} renamed", plural(n)));
    }
    if parts.is_empty() {
        parts.push("no files".into());
    }
    format!(
        "{} · {} · {} · {}{}",
        when(lb.committed_at),
        short_batch(lb.batch),
        parts.join(", "),
        crate::ui::preview::human_size(bytes),
        if lb.undone { " · undone" } else { "" }
    )
}

/// Source → destination in monospace (destination under the root shortened to `…\`), with the
/// sha256 prefix and any error.
fn path_job(ui: &egui::Ui, r: &OpRecord, root: Option<&Path>) -> egui::text::LayoutJob {
    let p = pal(ui);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let short = |path: &Path| {
        root.and_then(|rt| path.strip_prefix(rt).ok()).map_or_else(
            || path.display().to_string(),
            |rel| format!("…\\{}", rel.display()),
        )
    };
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat::simple(font.clone(), p.text);
    match (&r.src, &r.dst) {
        (Some(s), Some(d)) => {
            job.append(&s.display().to_string(), 0.0, plain.clone());
            job.append(&format!("\n→ {}", short(d)), 0.0, plain.clone());
        }
        (Some(s), None) => job.append(&s.display().to_string(), 0.0, plain.clone()),
        (None, Some(d)) => job.append(&short(d), 0.0, plain.clone()),
        (None, None) => {}
    }
    if let Some(h) = &r.sha256
        && h.len() > 6
    {
        job.append(
            &format!("sha256 {}…{}", &h[..4], &h[h.len() - 2..]),
            8.0,
            egui::TextFormat::simple(font.clone(), p.muted),
        );
    }
    if let Some(e) = &r.error {
        job.append(
            &format!("\n{e}"),
            0.0,
            egui::TextFormat::simple(egui::FontId::proportional(12.0), p.pill(PillKind::Err).1),
        );
    }
    job
}

fn history_widths(total: f32) -> [f32; 6] {
    let gaps = 5.0 * 8.0;
    let free = (total - 90.0 - 170.0 - 70.0 - 100.0 - gaps).max(300.0);
    [90.0, 170.0, free * 0.30, free * 0.70, 70.0, 100.0]
}

/// A fixed-width table cell.
fn hcell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

/// Why Undo is not possible now, if it isn't. During a commit the last batch on record is still
/// the previous one, so undo waits for the commit to finish.
fn undo_blocker(state: &AppState) -> Option<String> {
    if state.committing {
        return Some("Wait for the commit to finish.".into());
    }
    undo::blocker(state.last_batch.as_ref())
}

/// The op as written in ops.jsonl (UI_SPEC §13 shows these names in the pills).
fn op_name(op: Op) -> &'static str {
    match op {
        Op::Move => "move",
        Op::CopyVerifyDelete => "copy_verify_delete",
        Op::Mkdir => "mkdir",
        Op::Merge => "merge",
        Op::Split => "split",
        Op::Undo => "undo",
        Op::Skip => "skip",
        Op::Sentinel => "sentinel",
        Op::Backup => "backup",
        Op::SentinelSetAside => "sentinel_set_aside",
        Op::Copy => "copy",
        Op::Rollback => "rollback",
        Op::ZipImport => "zip_import",
        Op::ZipEntry => "zip_entry",
        Op::RenameDir => "rename_dir",
    }
}

fn short_batch(b: Uuid) -> String {
    b.simple().to_string()[..8].to_string()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
