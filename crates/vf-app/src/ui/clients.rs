//! Client master UI (PHASES P01 step 3): new-client wizard, CSV import, and the client panel
//! (entity type, relationships) shown in the Browse inspector.

use std::path::PathBuf;

use egui::RichText;
use filer_core::clients::{CsvTable, apply_mapping, read_csv};
use filer_core::config::ColumnMap;
use filer_core::relationships::ROLES;
use vf_fs::clients::{self, ENTITY_TYPES, ImportPlan, NewClient};

use crate::ui::widgets::{banner, field, mono, muted, primary, section};

/// Result of a dialog frame.
pub enum Outcome<T> {
    Open,
    Cancel,
    Done(T),
}

fn modal<R>(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Modal::new(egui::Id::unique(id))
        .show(ctx, |ui| {
            ui.set_width(560.0_f32.min(ctx.content_rect().width() - 48.0));
            add(ui)
        })
        .inner
}

fn entity_combo(ui: &mut egui::Ui, id: &str, label: egui::Id, value: &mut Option<String>) -> bool {
    let before = value.clone();
    let text = value.as_deref().map_or("Not set", clients::entity_label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "Not set");
            for (k, l) in ENTITY_TYPES {
                ui.selectable_value(value, Some(k.to_owned()), l);
            }
        })
        .response
        .labelled_by(label);
    *value != before
}

// ---------------------------------------------------------------------------
// New client

#[derive(Default)]
pub struct NewClientDialog {
    pub client: NewClient,
    pub error: Option<String>,
}

impl NewClientDialog {
    /// `Done` when the user confirms; the caller commits and reports errors back via `error`.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        tpl: &filer_core::template::TemplateNode,
    ) -> Outcome<NewClient> {
        modal(ctx, "new_client", |ui| {
            ui.label(RichText::new("New client").size(16.0).strong());
            field(
                ui,
                "Client ID (as in your tax software)",
                egui::TextEdit::singleline(&mut self.client.ext_id).desired_width(200.0),
            );
            field(
                ui,
                "Client name",
                egui::TextEdit::singleline(&mut self.client.name).desired_width(f32::INFINITY),
            );
            let l = ui.label("Entity type");
            entity_combo(ui, "nc_entity", l.id, &mut self.client.entity_type);
            section(ui, "Folders to create");
            let ready =
                !self.client.ext_id.trim().is_empty() && !self.client.name.trim().is_empty();
            if ready {
                match clients::preview_folders(tpl, &self.client) {
                    Ok(dirs) => {
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .show(ui, |ui| {
                                for d in dirs {
                                    ui.label(mono(d).small());
                                }
                            });
                        ui.label(muted(
                            "Year folders are created when the first document for that year is filed.",
                        ));
                    }
                    Err(e) => banner(ui, &e.to_string(), true),
                }
            } else {
                ui.label(muted("Enter the ID and name to see the folders."));
            }
            if let Some(e) = &self.error {
                banner(ui, e, true);
            }
            let mut out = Outcome::Open;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(ready, primary("Create client (one transaction)"))
                    .clicked()
                {
                    out = Outcome::Done(self.client.clone());
                }
                if ui.button("Cancel").clicked() {
                    out = Outcome::Cancel;
                }
            });
            out
        })
    }
}

// ---------------------------------------------------------------------------
// CSV import

pub enum ImportStep {
    Preview,
    Commit(bool),
}

pub struct ImportDialog {
    pub path: PathBuf,
    table: Result<CsvTable, String>,
    map: ColumnMap,
    pub plan: Option<Result<ImportPlan, String>>,
    pub create_missing: bool,
    pub error: Option<String>,
}

fn guess(headers: &[String], words: &[&str]) -> Option<String> {
    headers
        .iter()
        .find(|h| words.contains(&h.trim().to_lowercase().as_str()))
        .cloned()
}

impl ImportDialog {
    pub fn new(path: PathBuf) -> Self {
        let table = read_csv(&path).map_err(|e| e.to_string());
        let mut map = ColumnMap::default();
        if let Ok(t) = &table {
            map.id = guess(
                &t.headers,
                &["id", "client id", "client_id", "client number", "number"],
            );
            map.name = guess(
                &t.headers,
                &["name", "client name", "client_name", "client"],
            );
            map.extras = t
                .headers
                .iter()
                .filter(|h| Some(*h) != map.id.as_ref() && Some(*h) != map.name.as_ref())
                .cloned()
                .collect();
        }
        Self {
            path,
            table,
            map,
            plan: None,
            create_missing: false,
            error: None,
        }
    }

    /// The client list from the current mapping (Filer's `apply_mapping`).
    pub fn list(&self) -> Result<filer_core::clients::ClientList, String> {
        let t = self.table.as_ref().map_err(Clone::clone)?;
        apply_mapping(t, &self.map).map_err(|e| e.to_string())
    }

    /// `Done(Preview)` asks the caller to plan the import; `Done(Commit(create_missing))` to
    /// apply the shown plan.
    pub fn ui(&mut self, ctx: &egui::Context) -> Outcome<ImportStep> {
        modal(ctx, "import_clients", |ui| {
            ui.label(RichText::new("Import clients from CSV").size(16.0).strong());
            ui.label(mono(self.path.display().to_string()).small());
            let mut out = Outcome::Open;
            let headers = match &self.table {
                Ok(t) => t.headers.clone(),
                Err(e) => {
                    banner(ui, &format!("Cannot read this CSV: {e}"), true);
                    if ui.button("Close").clicked() {
                        out = Outcome::Cancel;
                    }
                    return out;
                }
            };
            let before = self.map.clone();
            egui::Grid::new("import_map").num_columns(2).show(ui, |ui| {
                for (label, slot) in [
                    ("ID column", &mut self.map.id),
                    ("Name column", &mut self.map.name),
                ] {
                    let l = ui.label(label);
                    egui::ComboBox::from_id_salt(label)
                        .selected_text(slot.clone().unwrap_or_else(|| "Choose…".into()))
                        .show_ui(ui, |ui| {
                            for h in &headers {
                                ui.selectable_value(slot, Some(h.clone()), h);
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    ui.end_row();
                }
            });
            ui.label(muted(
                "Every other column becomes a {token} for folder and file names (Filer Desktop).",
            ));
            if self.map != before {
                self.map.extras = headers
                    .iter()
                    .filter(|h| {
                        Some(*h) != self.map.id.as_ref() && Some(*h) != self.map.name.as_ref()
                    })
                    .cloned()
                    .collect();
                self.plan = None;
            }
            match &self.plan {
                Some(Ok(p)) => {
                    section(ui, "What will happen");
                    ui.label(format!(
                        "{} new · {} updated · {} existing folders bound · {} without a folder",
                        p.new_count(),
                        p.changed_count(),
                        p.discovered_count(),
                        p.missing_count()
                    ));
                    if p.skipped > 0 {
                        ui.label(muted(format!(
                            "{} rows skipped (no ID or name).",
                            p.skipped
                        )));
                    }
                    ui.checkbox(
                        &mut self.create_missing,
                        format!(
                            "Create folders from the template for the {} without one",
                            p.missing_count()
                        ),
                    );
                }
                Some(Err(e)) => banner(ui, e, true),
                None => {}
            }
            if let Some(e) = &self.error {
                banner(ui, e, true);
            }
            ui.horizontal(|ui| {
                let ready = matches!(self.plan, Some(Ok(_)));
                if !ready {
                    if ui.add(primary("Preview")).clicked() {
                        out = Outcome::Done(ImportStep::Preview);
                    }
                } else if ui.add(primary("Import (one transaction)")).clicked() {
                    out = Outcome::Done(ImportStep::Commit(self.create_missing));
                }
                if ui.button("Cancel").clicked() {
                    out = Outcome::Cancel;
                }
            });
            out
        })
    }
}

// ---------------------------------------------------------------------------
// Client panel

/// Edits the panel asks for.
pub enum ClientEdit {
    EntityType(Option<String>),
    AddRel {
        parent: String,
        child: String,
        kind: String,
        share: Option<f64>,
        note: String,
    },
    RemoveRel {
        parent: String,
        child: String,
        kind: String,
    },
}

#[derive(Default)]
pub struct RelForm {
    pub other: Option<String>,
    pub role: String,
    pub this_is_parent: bool,
    pub share: String,
    pub note: String,
    pub error: Option<String>,
}

/// The selected client's details. `others` are the clients offered as the related client.
pub fn client_panel(
    ui: &mut egui::Ui,
    client: &vf_core::browse::ClientRow,
    rels: &[clients::RelRow],
    others: &[vf_core::browse::ClientRow],
    form: &mut RelForm,
) -> Option<ClientEdit> {
    let mut edit = None;
    section(ui, "Client");
    ui.label(RichText::new(&client.name).strong());
    egui::Grid::new("client_kv").num_columns(2).show(ui, |ui| {
        ui.label(muted("ID"));
        ui.label(mono(&client.ext_id));
        ui.end_row();
        ui.label(muted("Folder"));
        ui.label(mono(client.folder_rel.as_deref().unwrap_or("none yet")).small());
        ui.end_row();
        ui.label(muted("Documents"));
        ui.label(client.docs.to_string());
        ui.end_row();
        let l = ui.label(muted("Entity type"));
        let mut et = client.entity_type.clone();
        if entity_combo(ui, "client_entity", l.id, &mut et) {
            edit = Some(ClientEdit::EntityType(et));
        }
        ui.end_row();
    });
    section(ui, "Relationships");
    if rels.is_empty() {
        ui.label(muted("None."));
    }
    for r in rels {
        ui.horizontal_wrapped(|ui| {
            let share = r.share_pct.map(|s| format!(" {s}%")).unwrap_or_default();
            ui.label(format!(
                "{} → {} ({}{share})",
                r.parent_name, r.child_name, r.kind
            ));
            if !r.note.is_empty() {
                ui.label(muted(&r.note));
            }
            if ui.small_button("Remove").clicked() {
                edit = Some(ClientEdit::RemoveRel {
                    parent: r.parent_id.clone(),
                    child: r.child_id.clone(),
                    kind: r.kind.clone(),
                });
            }
        });
    }
    ui.add_space(4.0);
    ui.label(muted("Add a relationship"));
    let other_name = form
        .other
        .as_ref()
        .and_then(|id| others.iter().find(|o| &o.id == id))
        .map_or("Related client…".to_owned(), |o| {
            format!("{} ({})", o.name, o.ext_id)
        });
    let lo = ui.label("Related client");
    egui::ComboBox::from_id_salt("rel_other")
        .selected_text(other_name)
        .width(260.0)
        .show_ui(ui, |ui| {
            for o in others.iter().filter(|o| o.id != client.id) {
                ui.selectable_value(
                    &mut form.other,
                    Some(o.id.clone()),
                    format!("{} ({})", o.name, o.ext_id),
                );
            }
        })
        .response
        .labelled_by(lo.id);
    ui.horizontal(|ui| {
        let lr = ui.label("Role");
        egui::ComboBox::from_id_salt("rel_role")
            .selected_text(if form.role.is_empty() {
                "Role…"
            } else {
                &form.role
            })
            .show_ui(ui, |ui| {
                for r in ROLES {
                    ui.selectable_value(&mut form.role, r.to_owned(), r);
                }
            })
            .response
            .labelled_by(lr.id);
        let l = ui.label("Share %");
        ui.add(egui::TextEdit::singleline(&mut form.share).desired_width(48.0))
            .labelled_by(l.id);
    });
    ui.checkbox(
        &mut form.this_is_parent,
        format!("{} is the entity (parent)", client.name),
    );
    field(
        ui,
        "Note",
        egui::TextEdit::singleline(&mut form.note).desired_width(f32::INFINITY),
    );
    if let Some(e) = &form.error {
        banner(ui, e, true);
    }
    if ui.button("Add relationship").clicked() {
        let share = form.share.trim();
        let share = if share.is_empty() {
            Ok(None)
        } else {
            share.trim_end_matches('%').parse::<f64>().map(Some)
        };
        match (&form.other, share) {
            (None, _) => form.error = Some("Choose the related client.".into()),
            (_, Err(_)) => form.error = Some("The share must be a number, like 40.".into()),
            (Some(other), Ok(share)) => {
                let (parent, child) = if form.this_is_parent {
                    (client.id.clone(), other.clone())
                } else {
                    (other.clone(), client.id.clone())
                };
                edit = Some(ClientEdit::AddRel {
                    parent,
                    child,
                    kind: form.role.clone(),
                    share,
                    note: form.note.clone(),
                });
            }
        }
    }
    edit
}
