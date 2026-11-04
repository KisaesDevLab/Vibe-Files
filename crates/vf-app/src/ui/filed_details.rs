//! "Just filed" (owner 2026-11-04: "need option to define tax year, status, source for each file
//! imported"): after Intake files a batch (Preview or a zip Import), the documents it filed are
//! listed above Filer's screen with their doc type, tax year, status and source, each editable,
//! and a "Set all" row for the whole batch. Apply writes the changes the way Browse's inspector
//! does (`vf_fs::docmeta::edit`: catalog + `docmeta.jsonl`, undoable from Browse).

use std::path::Path;

use egui::RichText;
use vf_core::db::Connection;
use vf_fs::docmeta::{MetaEdit, SOURCES, STATUSES};

use crate::ui::{theme, widgets};

/// Most documents listed from one batch (a bigger batch is edited in Browse).
const MAX_ROWS: usize = 300;

#[derive(Debug, Clone, PartialEq)]
struct Values {
    doc_type: Option<String>,
    /// A year, `Permanent`, or empty (not set).
    year: String,
    status: String,
    source: Option<String>,
}

#[derive(Debug, Clone)]
struct Row {
    id: String,
    name: String,
    was: Values,
    now: Values,
}

/// The documents of Intake's last recorded batch, being given their details.
#[derive(Debug, Clone)]
pub struct JustFiled {
    rows: Vec<Row>,
    /// The batch had more documents than are listed.
    more: usize,
    all: Values,
    pub error: Option<String>,
}

fn year_text(y: Option<i32>) -> String {
    y.map_or_else(|| "Permanent".to_string(), |y| y.to_string())
}

/// `Some(None)` = Permanent; `None` = not a year.
fn parse_year(t: &str) -> Option<Option<i32>> {
    let t = t.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("permanent") {
        return Some(None);
    }
    t.parse::<i32>()
        .ok()
        .filter(|y| (1900..=2100).contains(y))
        .map(Some)
}

impl JustFiled {
    /// The documents Intake filed in its last recorded batch (they share one filing time).
    pub fn load(conn: &Connection) -> Option<JustFiled> {
        let mut st = conn
            .prepare(
                "SELECT id, rel_path, doc_type_id, tax_year, status, source FROM document
                 WHERE deleted_at IS NULL AND filed_by = 'intake'
                   AND filed_at = (SELECT max(filed_at) FROM document WHERE filed_by = 'intake')
                 ORDER BY rel_path",
            )
            .ok()?;
        let all: Vec<Row> = st
            .query_map([], |r| {
                let rel: String = r.get(1)?;
                let v = Values {
                    doc_type: r.get(2)?,
                    year: year_text(r.get(3)?),
                    status: r.get(4)?,
                    source: r.get(5)?,
                };
                Ok(Row {
                    id: r.get(0)?,
                    name: rel.rsplit('/').next().unwrap_or(&rel).to_string(),
                    was: v.clone(),
                    now: v,
                })
            })
            .ok()?
            .flatten()
            .collect();
        if all.is_empty() {
            return None;
        }
        let more = all.len().saturating_sub(MAX_ROWS);
        let rows: Vec<Row> = all.into_iter().take(MAX_ROWS).collect();
        let all = rows[0].now.clone();
        Some(JustFiled {
            rows,
            more,
            all,
            error: None,
        })
    }

    /// Writes the rows that changed. Returns how many documents changed.
    pub fn apply(
        &mut self,
        conn: &mut Connection,
        root: &Path,
        device_id: &str,
    ) -> Result<usize, String> {
        let mut n = 0;
        for r in &mut self.rows {
            if r.now == r.was {
                continue;
            }
            let year = parse_year(&r.now.year)
                .ok_or_else(|| format!("{}: {} is not a tax year", r.name, r.now.year))?;
            let e = MetaEdit {
                doc_type: (r.now.doc_type != r.was.doc_type).then(|| r.now.doc_type.clone()),
                tax_year: (r.now.year != r.was.year).then_some(year),
                status: (r.now.status != r.was.status).then(|| r.now.status.clone()),
                source: (r.now.source != r.was.source).then(|| r.now.source.clone()),
                tags: None,
            };
            vf_fs::docmeta::edit(conn, root, std::slice::from_ref(&r.id), &e, device_id)?;
            r.was = r.now.clone();
            n += 1;
        }
        Ok(n)
    }

    fn changed(&self) -> usize {
        self.rows.iter().filter(|r| r.now != r.was).count()
    }
}

/// What the panel asked for.
pub enum Act {
    None,
    Apply,
    Close,
}

fn status_box(ui: &mut egui::Ui, id: &str, v: &mut String) {
    let shown = STATUSES
        .iter()
        .find(|(k, _)| *k == v.as_str())
        .map_or(v.as_str(), |(_, l)| *l)
        .to_string();
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(110.0)
        .show_ui(ui, |ui| {
            for (k, l) in STATUSES {
                ui.selectable_value(v, k.to_string(), l);
            }
        });
}

fn source_box(ui: &mut egui::Ui, id: &str, v: &mut Option<String>) {
    let shown = v
        .as_deref()
        .and_then(|s| SOURCES.iter().find(|(k, _)| *k == s))
        .map_or("—", |(_, l)| *l)
        .to_string();
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(110.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(v, None, "—");
            for (k, l) in SOURCES {
                ui.selectable_value(v, Some(k.to_string()), l);
            }
        });
}

fn type_box(ui: &mut egui::Ui, id: &str, v: &mut Option<String>, types: &[(String, String)]) {
    let shown = v
        .as_deref()
        .and_then(|t| types.iter().find(|(i, _)| i == t))
        .map_or("Needs doc type", |(_, n)| n.as_str())
        .to_string();
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(170.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(v, None, "Needs doc type");
            for (i, n) in types {
                ui.selectable_value(v, Some(i.clone()), n);
            }
        });
}

/// The panel (a top panel on the Intake tab). `types`: (id, name) of the live doc types.
pub fn show(ui: &mut egui::Ui, jf: &mut JustFiled, types: &[(String, String)]) -> Act {
    let mut act = Act::None;
    egui::Panel::top("vf-just-filed")
        .resizable(true)
        .default_size(240.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANE)
                .inner_margin(egui::Margin::symmetric(16, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Just filed: {} document{}",
                        jf.rows.len() + jf.more,
                        if jf.rows.len() + jf.more == 1 { "" } else { "s" }
                    ))
                    .strong(),
                );
                ui.label(widgets::caption(
                    "Set each one's doc type, tax year, status and source; Undo in Browse takes a change back.",
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        act = Act::Close;
                    }
                    let n = jf.changed();
                    if ui
                        .add_enabled(n > 0, widgets::primary_button(format!("Apply ({n})")))
                        .clicked()
                    {
                        act = Act::Apply;
                    }
                });
            });
            if let Some(e) = &jf.error {
                ui.label(RichText::new(e).color(crate::ui::wizard::ERROR));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Set all").small().color(theme::INK_MUTED));
                type_box(ui, "jf-all-type", &mut jf.all.doc_type, types);
                let l = ui.label("Tax year");
                ui.add(egui::TextEdit::singleline(&mut jf.all.year).desired_width(70.0))
                    .labelled_by(l.id);
                status_box(ui, "jf-all-status", &mut jf.all.status);
                source_box(ui, "jf-all-source", &mut jf.all.source);
                if ui.button("Set all rows").clicked() {
                    for r in &mut jf.rows {
                        r.now = jf.all.clone();
                    }
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("jf-rows")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new("jf-grid")
                        .num_columns(5)
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for h in ["DOCUMENT", "DOC TYPE", "TAX YEAR", "STATUS", "SOURCE"] {
                                ui.label(RichText::new(h).small().color(theme::INK_MUTED));
                            }
                            ui.end_row();
                            for (i, r) in jf.rows.iter_mut().enumerate() {
                                ui.label(RichText::new(&r.name).monospace());
                                type_box(ui, &format!("jf-type-{i}"), &mut r.now.doc_type, types);
                                let bad = parse_year(&r.now.year).is_none();
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut r.now.year)
                                        .desired_width(70.0)
                                        .text_color_opt(bad.then_some(crate::ui::wizard::ERROR)),
                                );
                                resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::accesskit::Role::TextInput,
                                        true,
                                        format!("Tax year of {}", r.name),
                                    )
                                });
                                status_box(ui, &format!("jf-status-{i}"), &mut r.now.status);
                                source_box(ui, &format!("jf-source-{i}"), &mut r.now.source);
                                ui.end_row();
                            }
                        });
                    if jf.more > 0 {
                        ui.label(widgets::caption(format!(
                            "{} more in this batch: set them in Browse.",
                            jf.more
                        )));
                    }
                });
        });
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_parse() {
        assert_eq!(parse_year("2025"), Some(Some(2025)));
        assert_eq!(parse_year("permanent"), Some(None));
        assert_eq!(parse_year(""), Some(None));
        assert_eq!(parse_year("25x"), None);
    }

    /// The last batch's documents are listed and a change is written to the catalog.
    #[test]
    fn last_batch_is_listed_and_changes_apply() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(&root).unwrap();
        let mut conn =
            vf_core::db::open(&t.path().join("c.db"), vf_core::db::Storage::Local).unwrap();
        conn.execute_batch(
            "INSERT INTO document (id, rel_path, hash, size, status, filed_by, filed_at, device_id, updated_at, tax_year)
             VALUES ('old', 'A/old.pdf', 'h0', 1, 'received', 'intake', '2026-11-01T00:00:00Z', 'd', 't', 2024),
                    ('d1', 'A/chase april 2026.pdf', 'h1', 1, 'received', 'intake', '2026-11-04T10:00:00Z', 'd', 't', NULL),
                    ('d2', 'A/chase may 2026.pdf', 'h2', 1, 'received', 'intake', '2026-11-04T10:00:00Z', 'd', 't', NULL);",
        )
        .unwrap();
        let mut jf = JustFiled::load(&conn).unwrap();
        assert_eq!(jf.rows.len(), 2, "only the last batch");
        jf.all.year = "2026".into();
        jf.all.source = Some("third_party".into());
        for r in &mut jf.rows {
            r.now = jf.all.clone();
        }
        jf.rows[1].now.status = "final".into();
        assert_eq!(jf.apply(&mut conn, &root, "dev").unwrap(), 2);
        let got: Vec<(i32, String, Option<String>)> = conn
            .prepare(
                "SELECT tax_year, status, source FROM document WHERE id IN ('d1','d2') ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            got[0],
            (2026, "received".into(), Some("third_party".into()))
        );
        assert_eq!(got[1].1, "final");
        assert_eq!(jf.changed(), 0);
    }
}
