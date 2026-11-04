//! "Import clients CSV…" (P01 step 3): Filer's CSV reader and column map, then the master list
//! is saved (`vf_fs::clients::import_csv`, uuids kept) and the catalog synced in one
//! Transaction. Folders that match an imported client but have no sentinel are reported; binding
//! them is the folder import (P10).

use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::{RichText, TextEdit};
use filer_core::clients::{CsvTable, read_csv};
use filer_core::config::ColumnMap;
use vf_fs::clients;

use crate::ui::browse::Env;
use crate::ui::theme;
use crate::ui::widgets::{self, DialogOutcome};

pub struct ImportDialog {
    pub path: String,
    table: Option<CsvTable>,
    pub map: ColumnMap,
    pub error: Option<String>,
    /// The catalog's `ext_id → uuid` when the dialog opened, so re-imports keep identities.
    known: BTreeMap<String, uuid::Uuid>,
    /// A taxpayer ID column whose last 4 digits are kept (Q-27, P08).
    pub tin: Option<String>,
}

impl ImportDialog {
    pub fn new(known: BTreeMap<String, uuid::Uuid>) -> Self {
        Self {
            path: String::new(),
            table: None,
            map: ColumnMap::default(),
            error: None,
            known,
            tin: None,
        }
    }
}

/// Guesses the ID and name columns from usual header names.
fn guess(headers: &[String]) -> ColumnMap {
    let find = |names: &[&str]| {
        headers
            .iter()
            .find(|h| {
                let k: String = h
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect();
                names.contains(&k.as_str())
            })
            .cloned()
    };
    ColumnMap {
        id: find(&["clientid", "id", "clientnumber", "client", "number"]),
        name: find(&["name", "clientname", "displayname", "fullname"]),
        extras: vec![],
    }
}

fn column_combo(
    ui: &mut egui::Ui,
    id: &str,
    label: egui::Id,
    headers: &[String],
    v: &mut Option<String>,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(v.clone().unwrap_or_else(|| "Choose…".into()))
        .show_ui(ui, |ui| {
            for h in headers {
                ui.selectable_value(v, Some(h.clone()), h);
            }
        })
        .response
        .labelled_by(label);
}

pub fn show(ctx: &egui::Context, d: &mut ImportDialog, env: &mut Env) -> DialogOutcome<String> {
    let mut out = DialogOutcome::Open;
    egui::Modal::new(egui::Id::unique("import-clients")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading("Import clients CSV");
        ui.label(widgets::caption(
            "The client list from your tax software. Re-importing keeps every client's identity.",
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let l = ui.label("CSV file");
            ui.add(TextEdit::singleline(&mut d.path).desired_width(330.0))
                .labelled_by(l.id);
            if ui.button("Browse…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv", "txt"])
                    .pick_file()
            {
                d.path = p.display().to_string();
            }
            if ui.button("Read").clicked() {
                match read_csv(&PathBuf::from(d.path.trim())) {
                    Ok(t) => {
                        d.map = guess(&t.headers);
                        d.table = Some(t);
                        d.error = None;
                    }
                    Err(e) => {
                        d.table = None;
                        d.error = Some(e.to_string());
                    }
                }
            }
        });
        ui.add_space(8.0);
        let mut ready = false;
        widgets::fixed_body(ui, "import-body", 260.0, |ui| {
            let Some(t) = &d.table else {
                ui.label(RichText::new("Choose the CSV, then Read.").color(theme::ink_muted()));
                return;
            };
            ui.label(format!(
                "{} rows · {} columns · {}",
                t.rows.len(),
                t.headers.len(),
                t.encoding.label()
            ));
            egui::Grid::new("import-map")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let l = ui.label("Client ID column");
                    column_combo(ui, "import-id", l.id, &t.headers, &mut d.map.id);
                    ui.end_row();
                    let l = ui.label("Name column");
                    column_combo(ui, "import-name", l.id, &t.headers, &mut d.map.name);
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label(widgets::caption(
                "Other columns to keep (folder-name tokens)",
            ));
            for h in &t.headers {
                if Some(h) == d.map.id.as_ref() || Some(h) == d.map.name.as_ref() {
                    continue;
                }
                if clients::looks_like_tin(h) {
                    // Only the last 4 digits are kept, for matching forms to clients (P08).
                    let mut on = d.tin.as_deref() == Some(h.as_str());
                    let label = format!("{h}: keep the last 4 digits only");
                    if ui
                        .checkbox(&mut on, label)
                        .on_hover_text("Full taxpayer IDs are never copied into the client list. The last 4 digits help match W-2s, 1099s and K-1s to clients.")
                        .changed()
                    {
                        d.tin = on.then(|| h.clone());
                    }
                    continue;
                }
                let mut on = d.map.extras.contains(h);
                let r = ui.checkbox(&mut on, h);
                if r.changed() {
                    if on {
                        d.map.extras.push(h.clone());
                    } else {
                        d.map.extras.retain(|x| x != h);
                    }
                }
            }
            ready = d.map.id.is_some() && d.map.name.is_some();
        });
        ui.add_space(6.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.horizontal(|ui| {
            let n = d.table.as_ref().map(|t| t.rows.len()).unwrap_or(0);
            let go = ui.add_enabled(
                ready,
                widgets::primary_button(format!("Import {n} rows (one transaction)")),
            );
            if go.clicked() {
                match run(d, env) {
                    Ok(msg) => out = DialogOutcome::Done(msg),
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

fn run(d: &ImportDialog, env: &mut Env) -> Result<String, String> {
    let list = clients::import_csv_tin(
        env.root,
        &PathBuf::from(d.path.trim()),
        &d.map,
        &d.known,
        d.tin.as_deref(),
    )?;
    let rep = vf_fs::rebuild::sync_clients(env.conn, env.root, env.device_id, "clients_import")
        .map_err(|e| format!("The client list was saved but the catalog was not updated: {e}"))?;
    let mut msg = format!(
        "Imported {} clients ({} without a folder yet).",
        list.len(),
        rep.master_only
    );
    if list.skipped_rows > 0 {
        msg.push_str(&format!(
            " {}",
            filer_core::clients::skipped_text(list.skipped_rows)
        ));
    }
    if !rep.uuid_mismatches.is_empty() {
        msg.push_str(&format!(
            " IDs whose folder belongs to a different client record (folder kept): {}.",
            rep.uuid_mismatches.join(", ")
        ));
    }
    match clients::unbound_matches(env.root, &list) {
        Ok(m) if !m.is_empty() => {
            let names: Vec<_> = m.iter().map(|(f, id)| format!("{f} (ID {id})")).collect();
            msg.push_str(&format!(
                " {} folders without a sentinel match a client: {} (bind them with the folder import).",
                m.len(),
                names.join(", ")
            ));
        }
        Ok(_) => {}
        Err(e) => msg.push_str(&format!(" Folder scan failed: {e}")),
    }
    Ok(msg)
}
