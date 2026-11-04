//! Admin › Devices (PA-24): the workstations that have opened this cabinet (recorded in
//! `_Vibe/devices/` when each opens it), and taking an old one off the list.

use egui::RichText;

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

#[derive(Default)]
pub struct DevicesPage {
    pub notice: Option<String>,
    pub error: Option<String>,
}

pub fn show(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    ui.label(RichText::new("Devices").strong().size(16.0));
    ui.label(widgets::caption(
        "Workstations that have opened this cabinet. Each is recorded when it opens it; a PC \
         taken off the list comes back if it opens the cabinet again.",
    ));
    ui.add_space(8.0);
    let list = vf_fs::safeguards::devices(env.root);
    let mut remove = None;
    egui::Grid::new("devices")
        .num_columns(5)
        .striped(true)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for h in ["COMPUTER", "VERSION", "FIRST SEEN", "LAST OPENED", ""] {
                ui.label(RichText::new(h).small().color(theme::ink_muted()));
            }
            ui.end_row();
            for d in &list {
                let this = d.id == env.device_id;
                ui.label(if this {
                    RichText::new(format!("{} (this PC)", d.name)).strong()
                } else {
                    RichText::new(&d.name)
                });
                ui.label(if d.version.is_empty() {
                    "—"
                } else {
                    &d.version
                });
                ui.label(widgets::local_time(&d.first_seen));
                ui.label(widgets::local_time(&d.last_seen));
                if this {
                    ui.label("");
                } else if ui
                    .small_button("Remove")
                    .on_hover_text("Take this PC off the list (it no longer opens the cabinet).")
                    .clicked()
                {
                    remove = Some(d.id.clone());
                }
                ui.end_row();
            }
        });
    if list.is_empty() {
        ui.label(widgets::caption("No workstation has been recorded yet."));
    }
    if let Some(id) = remove {
        match vf_fs::safeguards::remove_device(env.conn, env.root, env.device_id, &id) {
            Ok(()) => {
                p.notice = Some("Removed from the list.".into());
                p.error = None;
            }
            Err(e) => p.error = Some(e),
        }
    }
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink_secondary()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(theme::error()));
    }
}
