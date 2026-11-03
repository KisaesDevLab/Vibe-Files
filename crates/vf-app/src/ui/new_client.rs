//! `+ New client` (P01 step 3): ID, name, entity type → preview of the folder the cabinet
//! template gives → create in one Transaction (`vf_fs::newclient`).

use egui::{RichText, TextEdit};
use vf_fs::clients::ENTITY_TYPES;
use vf_fs::newclient::{self, NewClient, Plan};

use crate::ui::browse::Env;
use crate::ui::theme;
use crate::ui::widgets::{self, DialogOutcome};

#[derive(Default)]
pub struct NewClientDialog {
    pub req: NewClient,
    /// Recomputed whenever a field changes.
    plan: Option<Result<Plan, String>>,
    pub error: Option<String>,
}

impl NewClientDialog {
    pub fn new(ext_id: Option<String>) -> Self {
        Self {
            req: NewClient {
                ext_id: ext_id.unwrap_or_default(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// The new client's uuid and folder.
pub struct Created {
    pub client_id: String,
    pub folder: String,
}

pub fn show(ctx: &egui::Context, d: &mut NewClientDialog, env: &mut Env) -> DialogOutcome<Created> {
    if d.plan.is_none() {
        d.plan = Some(newclient::plan(env.conn, env.root, &d.req));
    }
    let mut out = DialogOutcome::Open;
    egui::Modal::new(egui::Id::unique("new-client")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading("New client");
        ui.add_space(8.0);
        let mut changed = false;
        let in_master = matches!(&d.plan, Some(Ok(p)) if p.in_master);
        egui::Grid::new("new-client-fields")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let l = ui.label("Client ID");
                changed |= ui
                    .add(TextEdit::singleline(&mut d.req.ext_id).desired_width(200.0))
                    .labelled_by(l.id)
                    .changed();
                ui.end_row();
                let l = ui.label("Client name");
                if in_master {
                    if let Some(Ok(p)) = &d.plan {
                        ui.label(format!("{} (from the client list)", p.client.name))
                            .labelled_by(l.id);
                    }
                } else {
                    changed |= ui
                        .add(TextEdit::singleline(&mut d.req.name).desired_width(320.0))
                        .labelled_by(l.id)
                        .changed();
                }
                ui.end_row();
                let l = ui.label("Entity type");
                egui::ComboBox::from_id_salt("entity-type")
                    .selected_text(widgets::label_of(
                        &ENTITY_TYPES,
                        d.req.entity_type.as_deref(),
                        "Not set",
                    ))
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(&mut d.req.entity_type, None, "Not set")
                            .changed();
                        for (k, l) in ENTITY_TYPES {
                            changed |= ui
                                .selectable_value(&mut d.req.entity_type, Some(k.to_string()), l)
                                .changed();
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();
            });
        if changed {
            d.plan = None;
            d.error = None;
        }
        ui.add_space(10.0);
        ui.label(widgets::caption("Creates"));
        let ready = widgets::fixed_body(ui, "new-client-preview", 220.0, |ui| match &d.plan {
            Some(Ok(p)) => {
                for dir in &p.dirs {
                    ui.label(RichText::new(dir).monospace().small());
                }
                ui.label(
                    RichText::new(format!("{}/_Vibe/client.json", p.folder_rel))
                        .monospace()
                        .small()
                        .color(theme::INK_MUTED),
                );
                true
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(theme::INK_MUTED));
                false
            }
            None => false,
        });
        ui.add_space(6.0);
        widgets::message_line(ui, d.error.as_deref());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let create = ui.add_enabled(
                ready,
                widgets::primary_button("Create client (one transaction)"),
            );
            if create.clicked()
                && let Some(Ok(p)) = &d.plan
            {
                match newclient::create(env.conn, env.root, p, env.device_id) {
                    Ok(_) => {
                        out = DialogOutcome::Done(Created {
                            client_id: p.client.uuid.to_string(),
                            folder: p.folder_rel.clone(),
                        })
                    }
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
