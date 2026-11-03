//! `+ New client` (P01 step 3): ID, name, entity type → preview of the folder the cabinet
//! template gives → create in one Transaction (`vf_fs::newclient`).

use egui::{RichText, TextEdit};
use vf_fs::clients::ENTITY_TYPES;
use vf_fs::newclient::{self, NewClient, Plan};

use crate::ui::browse::Env;
use crate::ui::{theme, wizard::ERROR};

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

pub enum Outcome {
    Open,
    Cancelled,
    /// The new client's uuid and folder.
    Created {
        client_id: String,
        folder: String,
    },
}

fn entity_label(key: Option<&str>) -> &'static str {
    key.and_then(|k| ENTITY_TYPES.iter().find(|(kk, _)| *kk == k))
        .map(|(_, l)| *l)
        .unwrap_or("Not set")
}

pub fn show(ctx: &egui::Context, d: &mut NewClientDialog, env: &mut Env) -> Outcome {
    if d.plan.is_none() {
        d.plan = Some(newclient::plan(env.conn, env.root, &d.req));
    }
    let mut out = Outcome::Open;
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
                    .selected_text(entity_label(d.req.entity_type.as_deref()))
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
        // Fixed-size preview and message area: a dialog whose height follows its content
        // re-centers over several frames and moves its buttons under the pointer.
        ui.label(RichText::new("Creates").small().color(theme::INK_MUTED));
        let ready = egui::ScrollArea::vertical()
            .id_salt("new-client-preview")
            .min_scrolled_height(220.0)
            .max_height(220.0)
            .auto_shrink([false, false])
            .show(ui, |ui| match &d.plan {
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
            })
            .inner;
        ui.add_space(6.0);
        let msg = d.error.as_deref().unwrap_or("");
        ui.add_sized(
            [ui.available_width(), 36.0],
            egui::Label::new(RichText::new(msg).color(ERROR)).wrap(),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let create = ui.add_enabled(
                ready,
                egui::Button::new(
                    RichText::new("Create client (one transaction)").color(egui::Color32::WHITE),
                )
                .fill(theme::ACCENT_DEFAULT),
            );
            if create.clicked()
                && let Some(Ok(p)) = &d.plan
            {
                match newclient::create(env.conn, env.root, p, env.device_id) {
                    Ok(_) => {
                        out = Outcome::Created {
                            client_id: p.client.uuid.to_string(),
                            folder: p.folder_rel.clone(),
                        }
                    }
                    Err(e) => d.error = Some(e),
                }
            }
            if ui.button("Cancel").clicked() {
                out = Outcome::Cancelled;
            }
        });
    });
    out
}
