//! CSV import wizard: pick the ID and name columns and which extra columns become template tokens,
//! with a preview of the first rows. Reading the file and the import itself run on worker threads;
//! the wizard stays open until the import is saved, so a failure keeps the user's choices.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};

use crate::clients::{self, CsvTable, ImportError};
use crate::config::{ColumnMap, Level};
use crate::ui::theme::banner;
use crate::ui::theme::{self, PillKind, label_caps, mono, pal, pill, strong};
use crate::worker;

const PREVIEW_ROWS: usize = 10;

pub enum Outcome {
    Open,
    Cancel,
    /// Start the import with this table and map. The wizard waits for [`CsvWizard::import_failed`]
    /// or is closed by the caller on success.
    Import {
        table: Arc<CsvTable>,
        map: ColumnMap,
    },
}

enum State {
    Loading(Receiver<Result<CsvTable, ImportError>>),
    Failed(String),
    Ready {
        table: Arc<CsvTable>,
        map: ColumnMap,
        import_error: Option<String>,
        importing: bool,
        /// Counts for the current map (recomputed when it changes).
        stats: Box<Option<(ColumnMap, Option<Stats>)>>,
    },
}

pub struct CsvWizard {
    path: PathBuf,
    initial: ColumnMap,
    state: State,
}

impl CsvWizard {
    /// Starts reading `path`. `initial` preselects columns that still exist in the file.
    pub fn open(ctx: &egui::Context, path: PathBuf, initial: ColumnMap) -> Self {
        let read_path = path.clone();
        let rx = worker::background(ctx, move || clients::read_csv(&read_path));
        Self {
            path,
            initial,
            state: State::Loading(rx),
        }
    }

    /// The background import failed: show why and let the user adjust or retry.
    pub fn import_failed(&mut self, msg: String) {
        if let State::Ready {
            import_error,
            importing,
            ..
        } = &mut self.state
        {
            *importing = false;
            *import_error = Some(format!("Cannot import: {msg}."));
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        if let State::Loading(rx) = &self.state
            && let Ok(result) = rx.try_recv()
        {
            self.state = match result {
                Ok(table) => {
                    let map = initial_map(&table.headers, &self.initial);
                    State::Ready {
                        table: Arc::new(table),
                        map,
                        import_error: None,
                        importing: false,
                        stats: Box::new(None),
                    }
                }
                Err(e) => State::Failed(e.to_string()),
            };
        }

        let mut outcome = Outcome::Open;
        let modal = egui::Modal::new(egui::Id::unique("csv_wizard"))
            .backdrop_color(egui::Color32::from_rgba_unmultiplied(27, 31, 36, 90))
            .show(ctx, |ui| {
                ui.set_width(860.0_f32.min(ctx.content_rect().width() - 48.0));
                ui.label(strong("Map CSV columns").size(16.0));
                let p = pal(ui);
                match &mut self.state {
                    State::Loading(_) => {
                        ui.label(mono(self.path.display().to_string()).color(p.muted));
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Reading CSV…");
                        });
                        if ui.button("Cancel").clicked() {
                            outcome = Outcome::Cancel;
                        }
                    }
                    State::Failed(msg) => {
                        ui.label(mono(self.path.display().to_string()).color(p.muted));
                        banner(ui, Level::Error, &format!("Cannot read this CSV: {msg}"));
                        if ui.button("Close").clicked() {
                            outcome = Outcome::Cancel;
                        }
                    }
                    State::Ready {
                        table,
                        map,
                        import_error,
                        importing,
                        stats,
                    } => {
                        outcome =
                            ready_ui(ui, &self.path, table, map, import_error, importing, stats);
                    }
                }
            });
        let importing = matches!(
            self.state,
            State::Ready {
                importing: true,
                ..
            }
        );
        if matches!(outcome, Outcome::Open) && modal.should_close() && !importing {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

/// Counts for the mapping pills and the Import button, recomputed when the map changes.
#[derive(Debug, Clone, PartialEq)]
struct Stats {
    /// Distinct non-blank IDs.
    unique_ids: usize,
    /// Rows skipped for a blank ID or name.
    blank: usize,
    /// IDs used by more than one row, sorted.
    duplicates: Vec<String>,
    /// Rows that would be imported.
    importable: usize,
}

fn compute_stats(table: &CsvTable, map: &ColumnMap) -> Option<Stats> {
    let col = |h: &Option<String>| {
        h.as_ref()
            .and_then(|h| table.headers.iter().position(|x| x == h))
    };
    let (id, name) = (col(&map.id)?, col(&map.name)?);
    let cell = |row: &[String], i: usize| row.get(i).map_or("", |c| c.trim()).to_string();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut blank = 0;
    for row in &table.rows {
        let (i, n) = (cell(row, id), cell(row, name));
        if i.is_empty() || n.is_empty() {
            blank += 1;
            continue;
        }
        *seen.entry(i).or_default() += 1;
    }
    let mut duplicates: Vec<String> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(id, _)| id.clone())
        .collect();
    duplicates.sort();
    Some(Stats {
        unique_ids: seen.len(),
        blank,
        duplicates,
        importable: table.rows.len() - blank,
    })
}

fn ready_ui(
    ui: &mut egui::Ui,
    path: &std::path::Path,
    table: &Arc<CsvTable>,
    map: &mut ColumnMap,
    import_error: &mut Option<String>,
    importing: &mut bool,
    stats: &mut Option<(ColumnMap, Option<Stats>)>,
) -> Outcome {
    let p = pal(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(mono(path.display().to_string()).color(p.muted));
        let mut tail = format!(
            " · {} rows · {} columns · header row detected",
            theme::thousands(table.rows.len()),
            table.headers.len()
        );
        if table.encoding == clients::Encoding::Windows1252 {
            tail.push_str(&format!(" · {}", table.encoding.label()));
        }
        ui.label(RichText::new(tail).color(p.muted));
    });
    ui.add_space(10.0);

    if stats.as_ref().is_none_or(|(m, _)| m != map) {
        *stats = Some((map.clone(), compute_stats(table, map)));
    }
    let current = stats.as_ref().and_then(|(_, s)| s.clone());

    let before = map.clone();
    ui.columns(2, |cols| {
        cols[0].set_max_width(340.0);
        cols[0].add_enabled_ui(!*importing, |ui| {
            mapping_ui(ui, table, map, current.as_ref());
        });
        preview_or_duplicates(&mut cols[1], table, map, current.as_ref());
    });
    if *map != before {
        *import_error = None;
    }

    let errors = clients::validate_mapping(&table.headers, map);
    for e in &errors {
        banner(ui, Level::Error, e);
    }
    if let Some(e) = import_error.as_ref() {
        banner(ui, Level::Error, e);
    }

    ui.add_space(6.0);
    ui.separator();
    let mut outcome = Outcome::Open;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Mapping is saved; later edits to the CSV re-import silently on launch.")
                .size(12.0)
                .color(p.muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let n = current.as_ref().map_or(0, |s| s.importable);
            let no_dups = current.as_ref().is_some_and(|s| s.duplicates.is_empty());
            let can_import = errors.is_empty() && import_error.is_none() && !*importing && no_dups;
            let label = format!(
                "Import {} client{}",
                theme::thousands(n),
                if n == 1 { "" } else { "s" }
            );
            if ui
                .add_enabled(can_import, theme::primary_button(ui, label))
                .clicked()
            {
                *importing = true;
                outcome = Outcome::Import {
                    table: Arc::clone(table),
                    map: map.clone(),
                };
            }
            if ui
                .add_enabled(!*importing, egui::Button::new("Cancel"))
                .clicked()
            {
                outcome = Outcome::Cancel;
            }
            if *importing {
                ui.spinner();
                ui.label("Importing…");
            }
        });
    });
    outcome
}

/// Required ID/name columns with their pills, then the token checkboxes.
fn mapping_ui(ui: &mut egui::Ui, table: &CsvTable, map: &mut ColumnMap, stats: Option<&Stats>) {
    let p = pal(ui);
    label_caps(ui, "Required");
    egui::Grid::new("csv_required")
        .num_columns(3)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            let id_label = ui.label("Client ID");
            column_combo(ui, "csv_id_col", &table.headers, &mut map.id, id_label.id);
            if let Some(s) = stats {
                if s.duplicates.is_empty() {
                    pill(
                        ui,
                        PillKind::Ok,
                        format!("{} unique", theme::thousands(s.unique_ids)),
                    );
                } else {
                    pill(
                        ui,
                        PillKind::Err,
                        format!("{} duplicate", theme::thousands(s.duplicates.len())),
                    );
                }
            }
            ui.end_row();
            let name_label = ui.label("Client name");
            column_combo(
                ui,
                "csv_name_col",
                &table.headers,
                &mut map.name,
                name_label.id,
            );
            if let Some(s) = stats {
                if s.blank == 0 {
                    pill(ui, PillKind::Ok, "no blanks");
                } else {
                    pill(
                        ui,
                        PillKind::Warn,
                        format!("{} blank → skipped", theme::thousands(s.blank)),
                    );
                }
            }
            ui.end_row();
        });
    ui.add_space(8.0);
    label_caps(ui, "Keep as template tokens");
    egui::ScrollArea::vertical()
        .id_salt("csv_extras")
        .max_height(200.0)
        .show(ui, |ui| {
            let mut ticked = Vec::new();
            for h in &table.headers {
                if h.is_empty() || is_id_or_name(map, h) {
                    continue;
                }
                let mut on = map.extras.contains(h);
                let token = clients::token_name(h);
                let mut job = egui::text::LayoutJob::default();
                let body = egui::TextStyle::Body.resolve(ui.style());
                job.append(h, 0.0, egui::TextFormat::simple(body, p.text));
                if !token.is_empty() {
                    job.append(
                        &format!(" → {{{token}}}"),
                        0.0,
                        egui::TextFormat::simple(
                            egui::TextStyle::Monospace.resolve(ui.style()),
                            p.muted,
                        ),
                    );
                }
                ui.checkbox(&mut on, job);
                if on {
                    ticked.push(h.clone());
                }
            }
            map.extras = merge_extras(&table.headers, map, &ticked);
        });
    ui.label(
        RichText::new("Token names are the headers reduced to letters, digits and underscores.")
            .size(11.0)
            .color(p.muted),
    );
}

/// The first rows with every column (mapped ones tinted, blank-ID rows red), or the duplicate
/// IDs in its place when there are any.
fn preview_or_duplicates(
    ui: &mut egui::Ui,
    table: &CsvTable,
    map: &ColumnMap,
    stats: Option<&Stats>,
) {
    let p = pal(ui);
    if let Some(s) = stats.filter(|s| !s.duplicates.is_empty()) {
        label_caps(ui, "Duplicate IDs");
        let (bg, fg) = p.pill(PillKind::Err);
        Frame::new()
            .fill(bg)
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!(
                        "These IDs are used by more than one row, so nothing can be imported: {}",
                        s.duplicates.join(", ")
                    ))
                    .color(fg),
                );
            });
        return;
    }
    label_caps(ui, &format!("First {PREVIEW_ROWS} rows"));
    let col_of = |h: &Option<String>| {
        h.as_ref()
            .and_then(|h| table.headers.iter().position(|x| x == h))
    };
    let (id_col, name_col) = (col_of(&map.id), col_of(&map.name));
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mono_font = egui::TextStyle::Monospace.resolve(ui.style());
    let head_font = egui::FontId::new(12.0, egui::FontFamily::Name(theme::SEMIBOLD.into()));
    let rows: Vec<&Vec<String>> = table.rows.iter().take(PREVIEW_ROWS).collect();
    // Column widths from the widest cell, clamped.
    let widths: Vec<f32> = (0..table.headers.len())
        .map(|c| {
            let mut w = ui
                .painter()
                .layout_no_wrap(table.headers[c].clone(), head_font.clone(), p.text)
                .size()
                .x;
            for r in &rows {
                let t = r.get(c).map_or("", |s| s.trim()).to_string();
                w = w.max(
                    ui.painter()
                        .layout_no_wrap(t, font.clone(), p.text)
                        .size()
                        .x,
                );
            }
            (w + 20.0).clamp(48.0, 200.0)
        })
        .collect();
    let row_h = 26.0;
    Frame::new()
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(4))
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("csv_preview")
                .show(ui, |ui| {
                    let total_w: f32 = widths.iter().sum();
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(total_w, row_h * (rows.len() + 1) as f32),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter_at(rect);
                    let mapped = |c: usize| Some(c) == id_col || Some(c) == name_col;
                    let token = |c: usize| map.extras.contains(&table.headers[c]);
                    let mut x = rect.left();
                    for (c, w) in widths.iter().enumerate() {
                        let tint = if mapped(c) {
                            Some(p.accent.gamma_multiply(0.10))
                        } else if token(c) {
                            Some(p.accent.gamma_multiply(0.04))
                        } else {
                            None
                        };
                        for r in 0..=rows.len() {
                            let cell = egui::Rect::from_min_size(
                                egui::pos2(x, rect.top() + r as f32 * row_h),
                                egui::vec2(*w, row_h),
                            );
                            let blank_id = r > 0
                                && id_col.is_some_and(|i| {
                                    rows[r - 1].get(i).is_none_or(|v| v.trim().is_empty())
                                });
                            if blank_id && Some(c) == id_col {
                                painter.rect_filled(cell, 0.0, p.pill(PillKind::Err).0);
                            } else if blank_id {
                                painter.rect_filled(cell, 0.0, p.tint_danger);
                            } else if let Some(t) = tint {
                                painter.rect_filled(cell, 0.0, t);
                            } else if r == 0 {
                                painter.rect_filled(cell, 0.0, p.tint_copy);
                            }
                            let (text, f, color) = if r == 0 {
                                (table.headers[c].clone(), head_font.clone(), p.muted)
                            } else {
                                let v = rows[r - 1].get(c).map_or("", |s| s.trim()).to_string();
                                let f = if Some(c) == id_col {
                                    mono_font.clone()
                                } else {
                                    font.clone()
                                };
                                (v, f, p.text)
                            };
                            let galley = painter.layout_no_wrap(text, f, color);
                            let clip = cell.shrink2(egui::vec2(8.0, 0.0));
                            painter.with_clip_rect(clip).galley(
                                egui::pos2(clip.left(), cell.center().y - galley.size().y / 2.0),
                                galley,
                                color,
                            );
                            painter.hline(
                                cell.x_range(),
                                cell.bottom(),
                                Stroke::new(1.0, p.border.gamma_multiply(0.6)),
                            );
                        }
                        x += w;
                    }
                });
        });
    ui.label(
        RichText::new(
            "Rows with a blank ID or name are skipped and counted. Duplicate IDs stop the import \
             and are listed.",
        )
        .size(11.0)
        .color(p.muted),
    );
}

fn is_id_or_name(map: &ColumnMap, h: &String) -> bool {
    Some(h) == map.id.as_ref() || Some(h) == map.name.as_ref()
}

/// New extras list in header order: the visible boxes that are ticked, plus earlier extras that
/// are hidden only because that column is currently the ID or name column. So briefly choosing a
/// ticked column as ID and switching back keeps it ticked.
fn merge_extras(headers: &[String], map: &ColumnMap, ticked: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in headers {
        let keep = if is_id_or_name(map, h) {
            map.extras.contains(h)
        } else {
            ticked.contains(h)
        };
        if keep && !out.contains(h) {
            out.push(h.clone());
        }
    }
    out
}

fn column_combo(
    ui: &mut egui::Ui,
    salt: &str,
    headers: &[String],
    value: &mut Option<String>,
    label: egui::Id,
) {
    egui::ComboBox::from_id_salt(salt)
        .width(150.0)
        .selected_text(value.as_deref().unwrap_or("(choose)"))
        .show_ui(ui, |ui| {
            for h in headers.iter().filter(|h| !h.is_empty()) {
                ui.selectable_value(value, Some(h.clone()), h);
            }
        })
        .response
        .labelled_by(label);
}

/// Saved columns when they still exist; otherwise a guess from common header names.
fn initial_map(headers: &[String], saved: &ColumnMap) -> ColumnMap {
    let has = |h: &String| headers.contains(h);
    let guess = |names: &[&str]| {
        headers
            .iter()
            .find(|h| names.contains(&h.trim().to_lowercase().as_str()))
            .cloned()
    };
    ColumnMap {
        id: saved.id.clone().filter(has).or_else(|| {
            guess(&[
                "id",
                "client id",
                "clientid",
                "client_id",
                "client #",
                "client number",
            ])
        }),
        name: saved
            .name
            .clone()
            .filter(has)
            .or_else(|| guess(&["name", "client name", "clientname", "client_name", "client"])),
        extras: saved.extras.iter().filter(|h| has(h)).cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn initial_map_prefers_saved_then_guesses() {
        let h = headers(&["Client ID", "Client Name", "Partner"]);
        let guessed = initial_map(&h, &ColumnMap::default());
        assert_eq!(guessed.id.as_deref(), Some("Client ID"));
        assert_eq!(guessed.name.as_deref(), Some("Client Name"));
        assert!(guessed.extras.is_empty());

        let saved = ColumnMap {
            id: Some("Client ID".into()),
            name: Some("Partner".into()),
            extras: vec!["Partner".into(), "Gone".into()],
        };
        let m = initial_map(&h, &saved);
        assert_eq!(m.name.as_deref(), Some("Partner"));
        assert_eq!(m.extras, vec!["Partner".to_string()]);
    }

    #[test]
    fn extra_survives_being_picked_as_id_briefly() {
        let h = headers(&["Client ID", "Client Name", "Partner", "Type"]);
        let mut map = ColumnMap {
            id: Some("Client ID".into()),
            name: Some("Client Name".into()),
            extras: vec!["Partner".into()],
        };
        // Partner chosen as ID: its box is hidden, Type ticked meanwhile.
        map.id = Some("Partner".into());
        map.extras = merge_extras(&h, &map, &["Type".into(), "Client ID".into()]);
        assert_eq!(map.extras, headers(&["Client ID", "Partner", "Type"]));
        // Back to Client ID: Partner is still ticked; Client ID box is hidden again.
        map.id = Some("Client ID".into());
        let ticked: Vec<String> = headers(&["Partner", "Type"]);
        map.extras = merge_extras(&h, &map, &ticked);
        assert!(map.extras.contains(&"Partner".to_string()));
        assert!(map.extras.contains(&"Type".to_string()));
    }
}
