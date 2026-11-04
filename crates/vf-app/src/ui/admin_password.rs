//! Admin › Admin password (PA-24, QUESTIONS Q-49): one password for the cabinet. It locks the
//! Admin tab and the Sync settings (push, pull and verify stay open to everyone), and is the
//! password that empties the trash and releases legal holds (it was the "firm PIN", Q-29).
//! Kept as an Argon2id hash in `_Vibe\trash_pin.json`; deleting that file clears a forgotten one.

use egui::{RichText, TextEdit};
use vf_fs::trash;

use crate::ui::browse::Env;
use crate::ui::{theme, widgets};

#[derive(Default)]
pub struct PasswordPage {
    current: String,
    new: String,
    again: String,
    pub notice: Option<String>,
    pub error: Option<String>,
}

fn secret(ui: &mut egui::Ui, label: &str, v: &mut String) {
    let l = ui.label(label);
    ui.add(TextEdit::singleline(v).password(true).desired_width(160.0))
        .labelled_by(l.id);
}

pub fn show(ui: &mut egui::Ui, p: &mut PasswordPage, env: &mut Env) {
    let has = trash::has_pin(env.root);
    ui.label(RichText::new("Admin password").strong().size(16.0));
    ui.label(widgets::caption(
        "One password for this cabinet. It locks the Admin tab and the Sync settings on every PC \
         (anyone can still push and pull), and it empties the trash and releases legal holds. \
         Forgotten: delete _Vibe\\trash_pin.json in the cabinet folder on the server.",
    ));
    ui.add_space(8.0);
    ui.label(if has {
        "An admin password is set."
    } else {
        "No admin password: anyone can change Admin and Sync settings."
    });
    ui.add_space(6.0);
    egui::Grid::new("admin-pass")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            if has {
                secret(ui, "Current password", &mut p.current);
                ui.end_row();
            }
            secret(ui, "New password", &mut p.new);
            ui.end_row();
            secret(ui, "New password again", &mut p.again);
            ui.end_row();
        });
    ui.horizontal(|ui| {
        let label = if has {
            "Change admin password"
        } else {
            "Set admin password"
        };
        if ui.add(widgets::primary_button(label)).clicked() {
            if p.new != p.again {
                p.error = Some("The two passwords differ.".into());
            } else {
                let current = has.then_some(p.current.as_str());
                match trash::set_pin(env.conn, env.root, env.device_id, current, &p.new) {
                    Ok(()) => {
                        tracing::info!("admin password set");
                        p.notice = Some("The admin password is set.".into());
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e),
                }
            }
            p.current.clear();
            p.new.clear();
            p.again.clear();
        }
        if has
            && ui
                .button("Remove admin password")
                .on_hover_text("Admin and Sync settings are open to everyone again.")
                .clicked()
        {
            match trash::remove_pin(env.conn, env.root, env.device_id, &p.current) {
                Ok(()) => {
                    p.notice = Some("The admin password is removed.".into());
                    p.error = None;
                }
                Err(e) => p.error = Some(e),
            }
            p.current.clear();
        }
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::ink_secondary()));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(theme::error()));
    }
}

/// What the lock screen asked for.
pub enum Unlock {
    None,
    /// The right password: unlocked.
    Unlocked,
}

/// The lock screen shown instead of Admin (and offered from Sync) while the password is set and
/// this session has not unlocked.
pub fn lock_screen(
    ui: &mut egui::Ui,
    typed: &mut String,
    msg: &mut Option<String>,
    env: &mut Env,
) -> Unlock {
    let mut out = Unlock::None;
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Admin is locked").strong().size(18.0));
        ui.label(widgets::caption(
            "Enter the admin password. It stays unlocked on this PC until Vibe Files closes.",
        ));
        ui.add_space(8.0);
        let l = ui.label("Admin password");
        let r = ui
            .add(
                TextEdit::singleline(typed)
                    .password(true)
                    .desired_width(220.0),
            )
            .labelled_by(l.id);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.add(widgets::primary_button("Unlock")).clicked() || enter) && !typed.is_empty() {
            match trash::unlock_admin(env.conn, env.root, env.device_id, typed) {
                Ok(true) => {
                    *msg = None;
                    out = Unlock::Unlocked;
                }
                Ok(false) => *msg = Some("That admin password is not right.".into()),
                Err(w) => *msg = Some(format!("Too many wrong passwords: try again in {w} s.")),
            }
            typed.clear();
        }
        if let Some(m) = msg {
            ui.label(RichText::new(m.as_str()).color(theme::error()));
        }
    });
    out
}
