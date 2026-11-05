//! Inspector for a selected client (P01 step 3): ID, folder, entity type and relationships
//! (entity ↔ shareholder / partner / beneficiary, share %). Edits go to the client master files
//! (`vf_fs::clients`) and then into the catalog with `rebuild::sync_clients`, one Transaction.

use egui::{RichText, TextEdit};
use filer_core::relationships::Relationship;
use vf_fs::clients::{self, ENTITY_TYPES};

use crate::ui::browse::{Browse, Env};
use crate::ui::new_client::NewClientDialog;
use crate::ui::theme;

/// The add-relationship form.
#[derive(Debug, Clone, Default)]
pub struct RelForm {
    pub other: String,
    /// This client is the parent (the entity); else the child (the owner).
    pub this_is_parent: bool,
    pub role: String,
    pub share: String,
    pub note: String,
    pub error: Option<String>,
}

impl RelForm {
    pub fn new() -> Self {
        Self {
            this_is_parent: true,
            ..Default::default()
        }
    }
}

/// Writes the catalog after a master-file change and re-reads Browse.
fn synced(b: &mut Browse, env: &mut Env, action: &str) {
    match vf_fs::rebuild::sync_clients(env.conn, env.root, env.device_id, action) {
        Ok(rep) => {
            b.reload(env.conn);
            if rep.dangling_relationships > 0 {
                b.notice = Some(format!(
                    "{} relationships name a client ID that is not in the cabinet.",
                    rep.dangling_relationships
                ));
            }
        }
        Err(e) => b.error = Some(format!("The catalog was not updated: {e}")),
    }
}

pub fn show(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env, ci: usize) {
    let c = b.snap.clients[ci].clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&c.name).strong().size(14.0));
        if ui
            .small_button("Rename client…")
            .on_hover_text("Change the client's name; its folder is renamed to match")
            .clicked()
        {
            b.folder_dialog = Some(crate::ui::folder_dialogs::FolderDialog::new(
                crate::ui::folder_dialogs::FolderAsk::RenameClient {
                    client_id: c.id.clone(),
                    name: c.name.clone(),
                },
            ));
        }
    });
    ui.label(
        RichText::new(format!("ID {}", c.ext_id))
            .monospace()
            .color(theme::ink_muted()),
    );
    ui.add_space(8.0);
    match &c.folder_rel {
        Some(f) => {
            ui.label(RichText::new("FOLDER").small().color(theme::ink_muted()));
            ui.label(RichText::new(f).monospace().small());
        }
        None => {
            ui.label(
                RichText::new("No folder yet (from the client list).").color(theme::ink_muted()),
            );
            if ui.button("Create folder…").clicked() {
                b.new_client = Some(NewClientDialog::new(Some(c.ext_id.clone())));
            }
        }
    }
    ui.add_space(10.0);

    let l = ui.label(
        RichText::new("ENTITY TYPE")
            .small()
            .color(theme::ink_muted()),
    );
    let mut et = c.entity_type.clone();
    egui::ComboBox::from_id_salt("client-entity-type")
        .selected_text(crate::ui::widgets::label_of(
            &ENTITY_TYPES,
            et.as_deref(),
            "Not set",
        ))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut et, None, "Not set");
            for (k, lbl) in ENTITY_TYPES {
                ui.selectable_value(&mut et, Some(k.to_string()), lbl);
            }
        })
        .response
        .labelled_by(l.id);
    if et != c.entity_type {
        match clients::set_entity_type(env.root, &c.id, et.as_deref()) {
            Ok(()) => synced(b, env, "client_edit"),
            Err(e) => b.error = Some(e),
        }
    }
    ui.add_space(12.0);

    ui.label(
        RichText::new("RELATIONSHIPS")
            .small()
            .color(theme::ink_muted()),
    );
    let name_of = |id: &str| {
        b.snap
            .clients
            .iter()
            .position(|x| x.id == id)
            .map(|i| {
                let x = &b.snap.clients[i];
                (x.name.clone(), x.ext_id.clone(), Some(i))
            })
            .unwrap_or_else(|| ("(unknown)".into(), "?".into(), None))
    };
    let mut remove: Option<(String, String, String)> = None;
    let mut open: Option<usize> = None;
    let rels: Vec<_> = b
        .snap
        .relationships
        .iter()
        .filter(|r| r.parent_id == c.id || r.child_id == c.id)
        .cloned()
        .collect();
    if rels.is_empty() {
        ui.label(RichText::new("None").color(theme::ink_muted()));
    }
    for r in &rels {
        let parent = r.parent_id == c.id;
        let (other, other_ext, other_ix) = name_of(if parent { &r.child_id } else { &r.parent_id });
        let share = r.share_pct.map(|p| format!(" · {p}%")).unwrap_or_default();
        let who = format!("{other} ({other_ext})");
        // The other client's name opens its folder (owner 2026-11-04).
        let mut name = |ui: &mut egui::Ui| match other_ix {
            Some(i) => {
                if ui
                    .link(&who)
                    .on_hover_text(format!("Open {other}'s folder"))
                    .clicked()
                {
                    open = Some(i);
                }
            }
            None => {
                ui.label(&who);
            }
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if parent {
                name(ui);
                ui.label(format!("is {}{share}", r.kind));
            } else {
                ui.label(format!("{} of", capitalize(&r.kind)));
                name(ui);
                if !share.is_empty() {
                    ui.label(share.trim_start());
                }
            }
            if ui
                .small_button("Remove")
                .on_hover_text("Remove this relationship")
                .clicked()
            {
                let (p_ext, c_ext) = if parent {
                    (c.ext_id.clone(), other_ext.clone())
                } else {
                    (other_ext.clone(), c.ext_id.clone())
                };
                remove = Some((p_ext, c_ext, r.kind.clone()));
            }
        });
    }
    if let Some(i) = open {
        b.go_to_client(i);
        return;
    }
    if let Some((p, ch, k)) = remove {
        match clients::remove_relationship(env.root, &p, &ch, &k) {
            Ok(()) => synced(b, env, "client_edit"),
            Err(e) => b.error = Some(e),
        }
    }

    ui.add_space(8.0);
    egui::CollapsingHeader::new("New relationship…")
        .id_salt("add-relationship")
        .show(ui, |ui| add_form(ui, b, env, &c));
}

fn capitalize(s: &str) -> String {
    let mut ch = s.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

fn add_form(ui: &mut egui::Ui, b: &mut Browse, env: &mut Env, c: &vf_core::browse::ClientRow) {
    let f = &mut b.rel_form;
    ui.horizontal(|ui| {
        ui.radio_value(&mut f.this_is_parent, true, "This client is the entity");
        ui.radio_value(&mut f.this_is_parent, false, "This client is the owner");
    });
    egui::Grid::new("rel-form")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            let other = if f.this_is_parent {
                "Owner client ID"
            } else {
                "Entity client ID"
            };
            let l = ui.label(other);
            ui.add(TextEdit::singleline(&mut f.other).desired_width(140.0))
                .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Role");
            ui.add(
                TextEdit::singleline(&mut f.role)
                    .hint_text("shareholder, partner, beneficiary…")
                    .desired_width(180.0),
            )
            .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Share %");
            ui.add(TextEdit::singleline(&mut f.share).desired_width(60.0))
                .labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Note");
            ui.add(TextEdit::singleline(&mut f.note).desired_width(180.0))
                .labelled_by(l.id);
            ui.end_row();
        });
    ui.label(
        RichText::new(format!(
            "Roles: {}",
            filer_core::relationships::ROLES.join(", ")
        ))
        .small()
        .color(theme::ink_muted()),
    );
    if let Some(e) = &f.error {
        ui.label(RichText::new(e).color(crate::ui::theme::error()));
    }
    if !ui.button("Add relationship").clicked() {
        return;
    }
    let other = f.other.trim().to_owned();
    if !b.snap.clients.iter().any(|x| x.ext_id == other) {
        b.rel_form.error = Some(format!("No client with ID \"{other}\" in this cabinet."));
        return;
    }
    let share = match f.share.trim() {
        "" => None,
        s => match s.trim_end_matches('%').trim().parse::<f64>() {
            Ok(v) => Some(v),
            Err(_) => {
                b.rel_form.error = Some("The share must be a number, like 40 or 33.3.".into());
                return;
            }
        },
    };
    let (parent_id, child_id) = if f.this_is_parent {
        (c.ext_id.clone(), other)
    } else {
        (other, c.ext_id.clone())
    };
    let rel = Relationship {
        parent_id,
        child_id,
        role: f.role.trim().to_owned(),
        note: f.note.trim().to_owned(),
    };
    match clients::add_relationship(env.root, rel, share) {
        Ok(()) => {
            b.rel_form = RelForm::new();
            synced(b, env, "client_edit");
        }
        Err(e) => b.rel_form.error = Some(e),
    }
}
