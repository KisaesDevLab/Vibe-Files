//! Admin › Devices (PA-24): the workstations that have opened this cabinet (recorded in
//! `_Vibe/devices/` when each opens it), and taking an old one off the list.

use egui::RichText;

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

/// Several Remote Desktop servers (PA-36): which one runs text recognition and B2 sync.
fn servers_section(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    use crate::server::Setting;
    ui.label(RichText::new("Servers").strong().size(16.0));
    // The choice for this computer (owner 2026-11-05): Windows decides, or always / never.
    let now = crate::server::mode_setting();
    let mut pick = now;
    ui.horizontal(|ui| {
        ui.label("Server mode on this computer");
        ui.radio_value(&mut pick, Setting::Auto, "Automatic")
            .on_hover_text(if crate::server::auto_detected() {
                "Windows Server running Vibe Files from a network folder: on."
            } else {
                "Not a Windows Server running Vibe Files from a network folder: off."
            });
        ui.radio_value(&mut pick, Setting::On, "Always on");
        ui.radio_value(&mut pick, Setting::Off, "Always off");
    });
    ui.label(widgets::caption(format!(
        "Now {} in this session. A change takes effect when Vibe Files next starts on this computer. Server mode: each login's settings stay on this computer, one catalog for all its logins, other servers' changes read, text recognition and B2 sync on one main server.",
        if crate::server::server_mode() { "on" } else { "off" }
    )));
    if pick != now {
        match crate::server::set_mode_setting(pick) {
            Ok(()) => {
                p.notice = Some(
                    "Server mode changes when Vibe Files next starts on this computer.".into(),
                );
                p.error = None;
            }
            Err(e) => p.error = Some(format!("Could not save it: {e}")),
        }
    }
    if !crate::server::server_mode() {
        ui.add_space(12.0);
        return;
    }
    ui.label(widgets::caption(
        "Vibe Files runs from a network folder, so each server keeps its own catalog of the cabinet and reads the other servers' changes (when a client is opened and every 15 minutes). Text recognition and B2 sync run on one of them.",
    ));
    let here = crate::server::computer();
    let main = crate::server::main_server(env.root);
    ui.horizontal(|ui| {
        ui.label(match &main {
            Some(m) if m.computer.eq_ignore_ascii_case(&here) => {
                format!("Text recognition and B2 sync run on this server ({here}).")
            }
            Some(m) => format!(
                "Text recognition and B2 sync run on {} (since {}).",
                m.computer,
                widgets::local_time(&m.since)
            ),
            None => "No server runs text recognition and B2 sync yet; the first to start Vibe Files takes it on.".into(),
        });
        let is_here = main
            .as_ref()
            .is_some_and(|m| m.computer.eq_ignore_ascii_case(&here));
        if !is_here
            && ui
                .button("Run them on this server")
                .on_hover_text("Takes effect when Vibe Files next starts on each server.")
                .clicked()
        {
            match crate::server::set_main(env.root) {
                Ok(()) => {
                    p.notice = Some(format!(
                        "Text recognition and B2 sync now run on {here} from the next start of Vibe Files."
                    ));
                    p.error = None;
                }
                Err(e) => p.error = Some(format!("Could not change it: {e}")),
            }
        }
    });
    ui.add_space(12.0);
}

/// What text recognition reads in this catalog (PA-34, Q-66).
fn reads_section(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    use vf_ocr::queue::{self, Reads};
    ui.label(RichText::new("Text recognition").strong().size(16.0));
    let Ok(now) = queue::reads(env.conn) else {
        return;
    };
    let mut all = now == Reads::All;
    ui.horizontal(|ui| {
        ui.radio_value(&mut all, true, "Read every PDF document");
        ui.radio_value(&mut all, false, "Read only documents filed from now on");
    });
    ui.label(widgets::caption(match &now {
        Reads::All => "Documents already in the cabinet are read too, a few at a time, Intake filings first.".to_string(),
        Reads::NewSince(t) => format!(
            "Documents filed since {} are read, through Vibe Files or put in the client folders outside it. The files a folder import found are searchable by name and details only.",
            widgets::local_time(t)
        ),
    }));
    if all != (now == Reads::All) {
        let to = if all {
            Reads::All
        } else {
            Reads::NewSince(vf_core::now_rfc3339())
        };
        match queue::set_reads(env.conn, &to) {
            Ok(()) => p.error = None,
            Err(e) => p.error = Some(format!("Could not change text recognition: {e}")),
        }
    }
}

/// This PC's shared catalog (Q-67: a Remote Desktop server's logins index the cabinet once).
fn shared_section(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    ui.label(RichText::new("This PC's catalog").strong().size(16.0));
    let Ok(cab) = vf_fs::cabinet::read(env.root) else {
        return;
    };
    match crate::shared::shared_catalog(&cab.cabinet_id) {
        Some(at) => {
            ui.label(format!(
                "Every Windows login of this PC uses one catalog for this cabinet: {}",
                at.display()
            ));
            ui.label(widgets::caption(
                "Each person keeps their own settings, undo and cache; changes made in one session show in the others within a few seconds.",
            ));
        }
        None => {
            ui.label(widgets::caption(
                "On a Remote Desktop server where several people use this cabinet, share this login's catalog: the others then use it instead of each reading the whole cabinet. It is kept on this PC's own disk (ProgramData).",
            ));
            if ui
                .button("Share this catalog with everyone on this PC")
                .clicked()
            {
                match crate::shared::share(env.conn, &cab.cabinet_id) {
                    Ok(at) => {
                        p.notice = Some(format!(
                            "Shared at {}. Close and open Vibe Files to use it; the other logins use it the next time they open the cabinet.",
                            at.display()
                        ));
                        p.error = None;
                    }
                    Err(e) => p.error = Some(format!("Not shared: {e:#}")),
                }
            }
        }
    }
}

#[derive(Default)]
pub struct DevicesPage {
    pub notice: Option<String>,
    pub error: Option<String>,
}

pub fn show(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    shared_section(ui, p, env);
    ui.add_space(12.0);
    reads_section(ui, p, env);
    ui.add_space(12.0);
    servers_section(ui, p, env);
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
