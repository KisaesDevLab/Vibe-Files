//! "Print cover sheet" (Browse toolbar) / "Cover sheet for client…" (Intake) and "Print 10
//! blank separators" (P02 step 3): writes the sheet PDF to `<data>/sheets/` and opens it in
//! the PDF app to print (Vibe's own Print is PA-13). **Several clients** (owner 2026-11-05,
//! Q-61): clients ticked in a searchable list, one doc type / tax year / sheet count for all
//! (each ticked row may set its own), one PDF with every client's sheets in list order.

use std::path::{Path, PathBuf};

use vf_core::browse::Snapshot;
use vf_ingest::sheet::{self, Cover, Labels, Sheet, TaxYear};

use crate::ui::widgets::{self, DialogOutcome};

pub struct CoverSheetDialog {
    /// (CSV ID, name, client folder) of every client with an ID.
    clients: Vec<(String, String, String)>,
    /// (id, name).
    doc_types: Vec<(String, String)>,
    /// Doc type id → default folder template.
    doc_type_folders: std::collections::HashMap<String, String>,
    /// How many sheets to print (one per document in the stack).
    pub copies: String,
    pub client: Option<usize>,
    pub doc_type: Option<usize>,
    /// A year, `Permanent`, or empty.
    pub year: String,
    pub error: Option<String>,
    /// Several clients at once, ticked in a searchable list.
    pub several: bool,
    /// The list's search: name or ID.
    pub find: String,
    /// One per client (the order of `clients`).
    pub rows: Vec<RowPick>,
    /// The row whose box was clicked last, for Shift-click runs.
    last_tick: Option<usize>,
}

/// A client's row in **Several clients**: ticked, and what it prints when not the settings
/// above (`None`: the setting above).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowPick {
    pub ticked: bool,
    /// `Some(None)`: no doc type.
    pub doc_type: Option<Option<usize>>,
    pub year: Option<String>,
    pub copies: Option<String>,
}

/// Sheets for one client: 1 to 50 (empty = 1).
fn copies_of(s: &str) -> Result<usize, String> {
    match s.trim() {
        "" => Ok(1),
        c => c
            .parse()
            .ok()
            .filter(|n| (1..=50).contains(n))
            .ok_or_else(|| "Sheets: a number from 1 to 50.".to_string()),
    }
}

impl CoverSheetDialog {
    /// `client`: the client uuid to start with; `doc_type`: the doc type id to start with (the
    /// cabinet's cover sheet default, Admin › Doc types). The tax year starts at last year: the
    /// return season's year (owner 2026-11-04).
    pub fn new(snap: &Snapshot, client: Option<&str>, doc_type: Option<&str>) -> Self {
        use chrono::Datelike;
        let clients: Vec<(String, String, String)> = snap
            .clients
            .iter()
            .filter(|c| !c.ext_id.is_empty())
            .map(|c| {
                (
                    c.ext_id.clone(),
                    c.name.clone(),
                    c.folder_rel.clone().unwrap_or_default(),
                )
            })
            .collect();
        let start = client.and_then(|id| {
            let c = snap.clients.iter().find(|c| c.id == id)?;
            clients.iter().position(|(e, _, _)| *e == c.ext_id)
        });
        let doc_types = snap.pickable_doc_types(None);
        // Its place in the list offered (archived types left out), QA 2026-11-04.
        let doc_type = doc_type.and_then(|id| doc_types.iter().position(|(t, _)| t == id));
        let n_clients = clients.len();
        CoverSheetDialog {
            clients,
            doc_types,
            doc_type_folders: snap.doc_type_folders.clone(),
            copies: "1".into(),
            client: start,
            doc_type,
            year: (chrono::Local::now().year() - 1).to_string(),
            error: None,
            several: false,
            find: String::new(),
            rows: vec![RowPick::default(); n_clients],
            last_tick: None,
        }
    }

    fn sheet(&self) -> Result<(Sheet, Labels), String> {
        let ci = self.client.ok_or("Choose the client.")?;
        self.sheet_for(ci, self.doc_type, &self.year)
    }

    /// Every ticked client's sheets, in list order, each with its own settings or the ones
    /// above.
    pub fn bulk_pages(&self) -> Result<Vec<(Sheet, Labels)>, String> {
        let mut out = vec![];
        for (i, r) in self.rows.iter().enumerate().filter(|(_, r)| r.ticked) {
            let who = || {
                let (id, n, _) = &self.clients[i];
                format!("{n} ({id})")
            };
            let dt = r.doc_type.unwrap_or(self.doc_type);
            let year = r.year.as_deref().unwrap_or(&self.year);
            let n = copies_of(r.copies.as_deref().unwrap_or(&self.copies))
                .map_err(|e| format!("{}: {e}", who()))?;
            let page = self
                .sheet_for(i, dt, year)
                .map_err(|e| format!("{}: {e}", who()))?;
            out.extend(std::iter::repeat_n(page, n));
        }
        if out.is_empty() {
            return Err("Tick at least one client.".into());
        }
        Ok(out)
    }

    /// How many clients are ticked.
    pub fn ticked(&self) -> usize {
        self.rows.iter().filter(|r| r.ticked).count()
    }

    /// The clients the search shows (indices into `clients`).
    fn shown(&self) -> Vec<usize> {
        let needle = self.find.trim().to_lowercase();
        (0..self.clients.len())
            .filter(|&i| {
                let (id, n, _) = &self.clients[i];
                needle.is_empty()
                    || n.to_lowercase().contains(&needle)
                    || id.to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn sheet_for(
        &self,
        ci: usize,
        doc_type: Option<usize>,
        year: &str,
    ) -> Result<(Sheet, Labels), String> {
        let (id, name, client_folder) = self.clients.get(ci).ok_or("Choose the client.")?;
        let doc_type = doc_type.and_then(|i| self.doc_types.get(i));
        let y = year.trim();
        let tax_year = if y.is_empty() {
            None
        } else if y.eq_ignore_ascii_case("permanent") {
            Some(TaxYear::Permanent)
        } else {
            match y.parse::<i32>() {
                Ok(n) if (1900..=2100).contains(&n) => Some(TaxYear::Year(n)),
                _ => {
                    return Err(format!(
                        "{y} is not a tax year (a year, Permanent, or empty)."
                    ));
                }
            }
        };
        Ok((
            Sheet(Some(Cover {
                client_id: id.clone(),
                doc_type: doc_type.map(|t| t.0.clone()),
                tax_year,
            })),
            Labels {
                client: format!("{name} ({id})"),
                folder: folder_label(
                    client_folder,
                    doc_type.and_then(|t| self.doc_type_folders.get(&t.0)),
                    tax_year,
                ),
                doc_type: doc_type.map(|t| t.1.clone()).unwrap_or_default(),
                tax_year: match tax_year {
                    Some(TaxYear::Year(n)) => n.to_string(),
                    Some(TaxYear::Permanent) => "Permanent".into(),
                    None => String::new(),
                },
            },
        ))
    }
}

/// The folder printed on a sheet: the client's folder, then the doc type's folder with the
/// year filled in (a `{Year}` with no year given stays as it is).
pub fn folder_label(
    client_folder: &str,
    template: Option<&String>,
    year: Option<TaxYear>,
) -> String {
    let mut parts: Vec<String> = vec![];
    if !client_folder.is_empty() {
        parts.push(client_folder.to_string());
    }
    if let Some(t) = template.filter(|t| !t.trim().is_empty()) {
        let t = match year {
            Some(TaxYear::Year(y)) => t.replace("{Year}", &y.to_string()),
            _ => t.clone(),
        };
        parts.extend(t.split('/').map(str::to_string));
    }
    parts.join("\\")
}

/// `<data>/sheets/<name>` (folder created).
fn out_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir.join(name))
}

/// Writes 10 blank separator sheets to `dir`. Returns the PDF.
pub fn separators(dir: &Path) -> Result<PathBuf, String> {
    let p = out_path(dir, "separator sheets.pdf")?;
    let pages: Vec<_> = (0..10)
        .map(|_| (Sheet::separator(), Labels::default()))
        .collect();
    sheet::write_pdf(&p, &pages)?;
    Ok(p)
}

/// The dialog; `Done` carries the written PDF.
pub fn show(ctx: &egui::Context, d: &mut CoverSheetDialog, dir: &Path) -> DialogOutcome<PathBuf> {
    let mut out = DialogOutcome::Open;
    egui::Modal::new(egui::Id::unique("cover-sheet")).show(ctx, |ui| {
        ui.set_width(if d.several { 760.0 } else { 460.0 });
        ui.heading(if d.several { "Cover sheets" } else { "Cover sheet" });
        ui.label(widgets::caption(
            "Put it on top of the document and scan the stack into the Scans folder: Intake splits the stack at each sheet and files the document to this client.",
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut d.several, false, "One client");
            ui.selectable_value(&mut d.several, true, "Several clients");
        });
        ui.add_space(6.0);
        egui::Grid::new("cover-fields")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                if !d.several {
                    let l = ui.label("Client");
                    let current = d
                        .client
                        .and_then(|i| d.clients.get(i))
                        .map(|(id, n, _)| format!("{n} ({id})"))
                        .unwrap_or_else(|| "Choose…".into());
                    let clients = &d.clients;
                    let (r, pick) = widgets::search_combo(
                        ui,
                        "cover-client",
                        300.0,
                        current,
                        || {
                            clients
                                .iter()
                                .map(|(id, n, _)| format!("{n} ({id})"))
                                .collect()
                        },
                        d.client,
                        None,
                    );
                    r.labelled_by(l.id);
                    if let Some(p) = pick {
                        d.client = p;
                    }
                    ui.end_row();
                }
                let l = ui.label("Doc type");
                let current = d
                    .doc_type
                    .and_then(|i| d.doc_types.get(i))
                    .map(|t| t.1.clone())
                    .unwrap_or_else(|| "Not set".into());
                let types = &d.doc_types;
                let (r, pick) = widgets::search_combo(
                    ui,
                    "cover-doc-type",
                    300.0,
                    current,
                    || types.iter().map(|t| t.1.clone()).collect(),
                    d.doc_type,
                    Some("Not set"),
                );
                r.labelled_by(l.id);
                if let Some(p) = pick {
                    d.doc_type = p;
                }
                ui.end_row();
                let l = ui.label("Tax year");
                ui.add(
                    egui::TextEdit::singleline(&mut d.year)
                        .hint_text("2025, Permanent, or empty")
                        .desired_width(160.0),
                )
                .labelled_by(l.id);
                ui.end_row();
                let l = ui.label(if d.several {
                    "Sheets per client"
                } else {
                    "Sheets"
                });
                ui.add(egui::TextEdit::singleline(&mut d.copies).desired_width(60.0))
                    .labelled_by(l.id)
                    .on_hover_text("One per document in the stack (each starts a new document).");
                ui.end_row();
            });
        if d.several {
            several_list(ui, d);
        } else if let Ok((_, l)) = d.sheet()
            && !l.folder.is_empty()
        {
            ui.label(widgets::caption(format!("Folder on the sheet: {}", l.folder)));
        }
        ui.add_space(4.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.horizontal(|ui| {
            let label = if d.several {
                format!("Create cover sheets ({} clients)", d.ticked())
            } else {
                "Create cover sheet".into()
            };
            if ui.add(widgets::primary_button(label)).clicked() {
                let r = if d.several {
                    d.bulk_pages().and_then(|pages| {
                        // Its own name: an earlier batch may still be open in the PDF app.
                        let p = out_path(
                            dir,
                            &format!(
                                "cover sheets {} clients {}.pdf",
                                d.ticked(),
                                chrono::Local::now().format("%Y-%m-%d %H%M%S")
                            ),
                        )?;
                        sheet::write_pdf(&p, &pages)?;
                        Ok(p)
                    })
                } else {
                    d.sheet().and_then(|(s, l)| {
                        let id = s.0.as_ref().map(|c| c.client_id.clone()).unwrap_or_default();
                        let p = out_path(
                            dir,
                            &format!("cover sheet {}.pdf", vf_ingest::pipeline::clean(&id)),
                        )?;
                        let n = copies_of(&d.copies)?;
                        sheet::write_pdf(&p, &vec![(s, l); n])?;
                        Ok(p)
                    })
                };
                match r {
                    Ok(p) => out = DialogOutcome::Done(p),
                    Err(e) => d.error = Some(e),
                }
            }
            if ui.button("Cancel").clicked() {
                out = DialogOutcome::Cancelled;
            }
        });
    });
    out
}

/// **Several clients**: search, tick (all shown, Shift-click runs), and a ticked row's own
/// doc type, year and sheets.
fn several_list(ui: &mut egui::Ui, d: &mut CoverSheetDialog) {
    ui.add_space(6.0);
    let shown = d.shown();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut d.find)
                .hint_text("Find client by name or ID")
                .desired_width(260.0),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::accesskit::Role::TextInput,
                true,
                "Find client by name or ID",
            )
        });
        if ui.button("Tick all shown").clicked() {
            for &i in &shown {
                d.rows[i].ticked = true;
            }
        }
        if ui.button("Untick all shown").clicked() {
            for &i in &shown {
                d.rows[i].ticked = false;
            }
        }
        ui.label(widgets::caption(format!(
            "{} ticked · showing {} of {}",
            d.ticked(),
            shown.len(),
            d.clients.len()
        )));
    });
    ui.label(widgets::caption(
        "A ticked client prints the settings above; change its doc type, year or sheets on its row to print something else for it. Shift-click ticks or unticks a run.",
    ));
    let shift = ui.input(|i| i.modifiers.shift);
    let mut clicked: Option<(usize, bool)> = None;
    let row_h = 28.0;
    egui::ScrollArea::vertical()
        .id_salt("cover-clients")
        // A fixed height: the dialog holds still while the search narrows the list.
        .min_scrolled_height(320.0)
        .max_height(320.0)
        .auto_shrink([false, false])
        .show_rows(ui, row_h, shown.len(), |ui, range| {
            for k in range {
                let i = shown[k];
                let (id, n, _) = d.clients[i].clone();
                ui.horizontal(|ui| {
                    ui.set_min_height(row_h - 4.0);
                    let r = ui.add_sized(
                        [300.0, row_h - 6.0],
                        egui::Checkbox::new(&mut d.rows[i].ticked, format!("{n} ({id})")),
                    );
                    if r.changed() {
                        clicked = Some((i, d.rows[i].ticked));
                    }
                    if !d.rows[i].ticked {
                        return;
                    }
                    let row = &mut d.rows[i];
                    let text = match row.doc_type {
                        None => "Same as above".to_string(),
                        Some(None) => "Not set".to_string(),
                        Some(Some(t)) => {
                            d.doc_types.get(t).map(|t| t.1.clone()).unwrap_or_default()
                        }
                    };
                    // "Not set", then the doc types; the `none` choice is "Same as above".
                    let types = &d.doc_types;
                    let current = row.doc_type.map(|t| t.map_or(0, |t| t + 1));
                    let (tr, pick) = widgets::search_combo(
                        ui,
                        ("cover-row-type", i),
                        170.0,
                        text,
                        || {
                            std::iter::once("Not set".to_string())
                                .chain(types.iter().map(|t| t.1.clone()))
                                .collect()
                        },
                        current,
                        Some("Same as above"),
                    );
                    tr.on_hover_text("This client's doc type").labelled_by(r.id);
                    if let Some(p) = pick {
                        row.doc_type = p.map(|k| k.checked_sub(1));
                    }
                    let mut y = row.year.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut y)
                                .hint_text(d.year.as_str())
                                .desired_width(80.0),
                        )
                        .on_hover_text("This client's tax year (empty: the year above)")
                        .labelled_by(r.id)
                        .changed()
                    {
                        row.year = (!y.trim().is_empty()).then_some(y);
                    }
                    let mut c = row.copies.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut c)
                                .hint_text(d.copies.as_str())
                                .desired_width(36.0),
                        )
                        .on_hover_text("Sheets for this client (empty: the number above)")
                        .labelled_by(r.id)
                        .changed()
                    {
                        row.copies = (!c.trim().is_empty()).then_some(c);
                    }
                });
            }
        });
    if let Some((i, on)) = clicked {
        if shift
            && let Some(a) = d.last_tick
            && let (Some(pa), Some(pi)) = (
                shown.iter().position(|&x| x == a),
                shown.iter().position(|&x| x == i),
            )
        {
            for &j in &shown[pa.min(pi)..=pa.max(pi)] {
                d.rows[j].ticked = on;
            }
        }
        d.last_tick = Some(i);
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn folder_on_the_sheet() {
        let tpl = "Income Tax/{Year}".to_string();
        assert_eq!(
            folder_label(
                "Bell Ranch (BELL2002)",
                Some(&tpl),
                Some(TaxYear::Year(2025))
            ),
            r"Bell Ranch (BELL2002)\Income Tax\2025"
        );
        assert_eq!(
            folder_label("Bell Ranch (BELL2002)", None, None),
            "Bell Ranch (BELL2002)"
        );
        assert_eq!(
            folder_label("Bell Ranch (BELL2002)", Some(&tpl), None),
            r"Bell Ranch (BELL2002)\Income Tax\{Year}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new cover sheet starts with the cabinet's default doc type and last year (owner
    /// 2026-11-04).
    #[test]
    fn starts_with_the_default_doc_type_and_last_year() {
        use chrono::Datelike;
        let snap = Snapshot {
            doc_types: vec![("t1".into(), "W-2".into()), ("t2".into(), "Misc".into())],
            ..Default::default()
        };
        let d = CoverSheetDialog::new(&snap, None, Some("t2"));
        assert_eq!(d.doc_type, Some(1));
        assert_eq!(d.year, (chrono::Local::now().year() - 1).to_string());
        // No default, or one that is gone: none picked.
        assert_eq!(CoverSheetDialog::new(&snap, None, None).doc_type, None);
        assert_eq!(
            CoverSheetDialog::new(&snap, None, Some("gone")).doc_type,
            None
        );
    }
}
