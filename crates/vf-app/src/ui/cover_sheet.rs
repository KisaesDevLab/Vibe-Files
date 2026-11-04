//! "Print cover sheet" (Browse toolbar) / "Cover sheet for client…" (Intake) and "Print 10
//! blank separators" (P02 step 3): writes the sheet PDF to `<data>/sheets/` and opens it in
//! the PDF app to print (Vibe's own Print is PA-13).

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
        CoverSheetDialog {
            clients,
            doc_types: snap.doc_types.clone(),
            doc_type_folders: snap.doc_type_folders.clone(),
            copies: "1".into(),
            client: start,
            doc_type: doc_type.and_then(|id| snap.doc_types.iter().position(|(t, _)| t == id)),
            year: (chrono::Local::now().year() - 1).to_string(),
            error: None,
        }
    }

    fn sheet(&self) -> Result<(Sheet, Labels), String> {
        let (id, name, client_folder) = self
            .client
            .and_then(|i| self.clients.get(i))
            .ok_or("Choose the client.")?;
        let doc_type = self.doc_type.and_then(|i| self.doc_types.get(i));
        let y = self.year.trim();
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
        ui.set_width(460.0);
        ui.heading("Cover sheet");
        ui.label(widgets::caption(
            "Put it on top of the document and scan the stack into the Scans folder: Intake splits the stack at each sheet and files the document to this client.",
        ));
        ui.add_space(8.0);
        egui::Grid::new("cover-fields")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let l = ui.label("Client");
                let current = d
                    .client
                    .and_then(|i| d.clients.get(i))
                    .map(|(id, n, _)| format!("{n} ({id})"))
                    .unwrap_or_else(|| "Choose…".into());
                egui::ComboBox::from_id_salt("cover-client")
                    .width(300.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (i, (id, n, _)) in d.clients.iter().enumerate() {
                            ui.selectable_value(&mut d.client, Some(i), format!("{n} ({id})"));
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();
                let l = ui.label("Doc type");
                let current = d
                    .doc_type
                    .and_then(|i| d.doc_types.get(i))
                    .map(|t| t.1.clone())
                    .unwrap_or_else(|| "Not set".into());
                egui::ComboBox::from_id_salt("cover-doc-type")
                    .width(300.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut d.doc_type, None, "Not set");
                        for (i, (_, n)) in d.doc_types.iter().enumerate() {
                            ui.selectable_value(&mut d.doc_type, Some(i), n);
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();
                let l = ui.label("Tax year");
                ui.add(
                    egui::TextEdit::singleline(&mut d.year)
                        .hint_text("2025, Permanent, or empty")
                        .desired_width(160.0),
                )
                .labelled_by(l.id);
                ui.end_row();
                let l = ui.label("Sheets");
                ui.add(egui::TextEdit::singleline(&mut d.copies).desired_width(60.0))
                    .labelled_by(l.id)
                    .on_hover_text("One per document in the stack (each starts a new document).");
                ui.end_row();
            });
        if let Ok((_, l)) = d.sheet()
            && !l.folder.is_empty()
        {
            ui.label(widgets::caption(format!("Folder on the sheet: {}", l.folder)));
        }
        ui.add_space(4.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.horizontal(|ui| {
            if ui
                .add(widgets::primary_button("Create cover sheet"))
                .clicked()
            {
                let r = d.sheet().and_then(|(s, l)| {
                    let id = s.0.as_ref().map(|c| c.client_id.clone()).unwrap_or_default();
                    let p = out_path(
                        dir,
                        &format!("cover sheet {}.pdf", vf_ingest::pipeline::clean(&id)),
                    )?;
                    let n: usize = match d.copies.trim() {
                        "" => 1,
                        c => c
                            .parse()
                            .ok()
                            .filter(|n| (1..=50).contains(n))
                            .ok_or("Sheets: a number from 1 to 50.")?,
                    };
                    sheet::write_pdf(&p, &vec![(s, l); n])?;
                    Ok(p)
                });
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
