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
use vf_fs::contacts::ContactColumns;

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
    /// The columns holding portal contacts (PA-105): saved by name only, never copied into
    /// the client list; Admin › Portal › Invite from client list reads them from the CSV.
    pub contacts: ContactColumns,
    /// The cabinet's client list and stored contacts when the CSV was read, for the preview.
    before: Option<(
        Option<filer_core::clients::ClientList>,
        BTreeMap<String, vf_fs::contacts::ContactRecord>,
    )>,
    /// What the import would change, for the columns it was worked out with (owner 2026-11-24:
    /// shown before anything changes).
    preview: Option<(
        ColumnMap,
        ContactColumns,
        Result<vf_fs::contacts::Preview, String>,
    )>,
    /// The catalog file: the import runs on a thread with its own connection (it reads every
    /// client folder); without it (tests), on this one.
    pub catalog: Option<PathBuf>,
    /// The import running (owner 2026-11-12: a spinner, not a frozen window).
    job: Option<crate::ui::job::Job<Result<String, String>>>,
    /// An admin password is set and Admin is locked: asked before importing (it also sets
    /// Intake › Setup's client CSV, which the password locks; owner 2026-11-12).
    pub needs_password: Option<bool>,
    pub password: String,
    /// Admin is unlocked in this window: no password asked.
    pub admin_unlocked: bool,
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
            contacts: ContactColumns::default(),
            before: None,
            preview: None,
            catalog: None,
            job: None,
            needs_password: None,
            password: String::new(),
            admin_unlocked: false,
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
        partner: None,
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
    let needs_password = *d
        .needs_password
        .get_or_insert_with(|| !d.admin_unlocked && vf_fs::trash::has_pin(env.root));
    if let Some(r) = crate::ui::job::take(&mut d.job) {
        match r.and_then(|r| r) {
            Ok(msg) => return DialogOutcome::Done(msg),
            Err(e) => d.error = Some(e),
        }
    }
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
                        // The columns chosen at the last import, where this CSV has them; else
                        // usual names.
                        let saved = vf_fs::contacts::columns(env.root).unwrap_or_default();
                        let keep = |c: &Option<String>| c.clone().filter(|h| t.headers.contains(h));
                        let saved = ContactColumns {
                            first: keep(&saved.first),
                            last: keep(&saved.last),
                            email: keep(&saved.email),
                            cell: keep(&saved.cell),
                            spouse_first: keep(&saved.spouse_first),
                            spouse_last: keep(&saved.spouse_last),
                            spouse_email: keep(&saved.spouse_email),
                            spouse_cell: keep(&saved.spouse_cell),
                        };
                        d.contacts = if saved.chosen().is_empty() {
                            let taken: Vec<&str> =
                                d.map.id.iter().chain(d.map.name.iter()).map(String::as_str).collect();
                            ContactColumns::guess(&t.headers, &taken)
                        } else {
                            saved
                        };
                        d.before = vf_fs::clients::load(env.root)
                            .ok()
                            .map(|m| (m.list, m.meta.contacts));
                        d.preview = None;
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
                    // The partner (or preparer) column, whatever its header (Filer V35).
                    let l = ui.label("Partner column");
                    egui::ComboBox::from_id_salt("import-partner")
                        .selected_text(d.map.partner.clone().unwrap_or_else(|| "None".into()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.map.partner, None, "None");
                            // A taxpayer ID column is never copied into the client list.
                            for h in t.headers.iter().filter(|h| !clients::looks_like_tin(h)) {
                                ui.selectable_value(&mut d.map.partner, Some(h.clone()), h);
                            }
                        })
                        .response
                        .labelled_by(l.id)
                        .on_hover_text("Browse's Partner filter and the {Partner} token read it");
                    ui.end_row();
                });
            ui.add_space(6.0);
            // PA-105 (owner 2026-11-24): the email and cell the portal invites with.
            egui::CollapsingHeader::new("Portal contacts (email and cell, for inviting clients to the portal)")
                .id_salt("import-contacts")
                .default_open(d.contacts.any())
                .show(ui, |ui| {
                    ui.label(widgets::caption(
                        "They are kept in the cabinet at Import (the CSV is not needed after) and updated by each Import; contacts typed in a client's panel are kept.",
                    ));
                    let headers: Vec<&String> = t
                        .headers
                        .iter()
                        .filter(|h| Some(*h) != d.map.id.as_ref() && Some(*h) != d.map.name.as_ref() && !clients::looks_like_tin(h))
                        .collect();
                    egui::Grid::new("import-contacts-grid")
                        .num_columns(2)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            let c = &mut d.contacts;
                            for (label, salt, v) in [
                                ("Taxpayer first name", "ic-first", &mut c.first),
                                ("Taxpayer last name", "ic-last", &mut c.last),
                                ("Email", "ic-email", &mut c.email),
                                ("Cell phone", "ic-cell", &mut c.cell),
                                ("Spouse first name", "ic-sfirst", &mut c.spouse_first),
                                ("Spouse last name", "ic-slast", &mut c.spouse_last),
                                ("Spouse email", "ic-semail", &mut c.spouse_email),
                                ("Spouse cell", "ic-scell", &mut c.spouse_cell),
                            ] {
                                let l = ui.label(label);
                                let none = if salt == "ic-slast" { "(same as taxpayer)" } else { "None" };
                                egui::ComboBox::from_id_salt(salt)
                                    .selected_text(v.clone().unwrap_or_else(|| none.into()))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(v, None, none);
                                        for h in &headers {
                                            ui.selectable_value(v, Some((*h).clone()), h.as_str());
                                        }
                                    })
                                    .response
                                    .labelled_by(l.id);
                                ui.end_row();
                            }
                        });
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
            ready = d.map.id.is_some()
                && d.map.name.is_some()
                && d.map.partner.as_ref().is_none_or(|p| Some(p) != d.map.id.as_ref() && Some(p) != d.map.name.as_ref());
            if ready {
                show_preview(ui, t, &d.map, &d.contacts, &d.before, &mut d.preview);
            }
        });
        ui.add_space(6.0);
        if needs_password {
            ui.horizontal(|ui| {
                let l = ui.label("Admin password");
                ui.add(
                    TextEdit::singleline(&mut d.password)
                        .password(true)
                        .desired_width(160.0),
                )
                .labelled_by(l.id)
                .on_hover_text("The import also sets Intake › Setup's client CSV, which the admin password locks");
            });
        }
        widgets::message_line(ui, d.error.as_deref());
        if let Some(j) = &d.job {
            j.show(ui);
            return;
        }
        ui.horizontal(|ui| {
            let n = d.table.as_ref().map(|t| t.rows.len()).unwrap_or(0);
            let go = ui.add_enabled(
                ready,
                widgets::primary_button(format!("Import {n} rows (one transaction)")),
            );
            // The admin password, checked (and wrong ones counted and slowed) as Admin's lock
            // screen does.
            let refused = go.clicked()
                && needs_password
                && match vf_fs::trash::unlock_admin(env.conn, env.root, env.device_id, &d.password) {
                    Ok(true) => {
                        d.needs_password = Some(false);
                        false
                    }
                    Ok(false) => {
                        d.error = Some(if vf_fs::trash::pin_unreadable(env.root) {
                            vf_fs::trash::PIN_UNREADABLE.into()
                        } else {
                            "That admin password is not right.".into()
                        });
                        true
                    }
                    Err(w) => {
                        d.error = Some(format!("Too many wrong passwords: try again in {w} s."));
                        true
                    }
                };
            if refused {
                d.password.clear();
            } else if go.clicked() {
                // A path every PC reaches (a mapped drive as \\server\share, made absolute):
                // it is saved in Intake's shared settings.
                let path = crate::portal::portable(
                    &std::path::absolute(d.path.trim())
                        .unwrap_or_else(|_| PathBuf::from(d.path.trim())),
                );
                let input = Input {
                    path,
                    map: d.map.clone(),
                    known: d.known.clone(),
                    tin: d.tin.clone(),
                    contacts: d.contacts.clone(),
                };
                match d.catalog.clone() {
                    Some(catalog) => {
                        let (root, device) = (env.root.to_path_buf(), env.device_id.to_string());
                        match crate::ui::job::Job::start(
                            ui.ctx(),
                            format!("Importing {n} clients and matching their folders…"),
                            move || {
                                let mut conn =
                                    vf_core::db::open(&catalog, vf_core::db::Storage::Local)
                                        .map_err(|e| e.to_string())?;
                                run(&input, &mut conn, &root, &device)
                            },
                        ) {
                            Ok(j) => d.job = Some(j),
                            Err(e) => d.error = Some(e),
                        }
                    }
                    None => match run(&input, env.conn, env.root, env.device_id) {
                        Ok(msg) => out = DialogOutcome::Done(msg),
                        Err(e) => d.error = Some(e),
                    },
                }
            }
            if ui.button("Cancel").clicked() {
                out = DialogOutcome::Cancelled;
            }
        });
    });
    out
}

/// "Changes in this CSV": worked out again when the columns change.
fn show_preview(
    ui: &mut egui::Ui,
    t: &CsvTable,
    map: &ColumnMap,
    cols: &ContactColumns,
    before: &Option<(
        Option<filer_core::clients::ClientList>,
        BTreeMap<String, vf_fs::contacts::ContactRecord>,
    )>,
    preview: &mut Option<(
        ColumnMap,
        ContactColumns,
        Result<vf_fs::contacts::Preview, String>,
    )>,
) {
    if preview
        .as_ref()
        .is_none_or(|(m, c, _)| m != map || c != cols)
    {
        let empty = BTreeMap::new();
        let (list, stored) = match before {
            Some((l, s)) => (l.as_ref(), s),
            None => (None, &empty),
        };
        *preview = Some((
            map.clone(),
            cols.clone(),
            vf_fs::contacts::preview(t, map, cols, list, stored),
        ));
    }
    let Some((_, _, p)) = preview else { return };
    ui.add_space(6.0);
    let p = match p {
        Ok(p) => p,
        Err(e) => {
            ui.label(RichText::new(e.as_str()).color(theme::warn()));
            return;
        }
    };
    if p.is_empty() {
        ui.label(widgets::caption("Changes in this CSV: none."));
        return;
    }
    const SHOWN: usize = 25;
    let more = |ui: &mut egui::Ui, n: usize| {
        if n > SHOWN {
            ui.label(widgets::caption(format!("… and {} more", n - SHOWN)));
        }
    };
    egui::CollapsingHeader::new(RichText::new(format!(
        "Changes in this CSV: {} new · {} leave the list · {} renamed · {} contacts",
        p.new.len(),
        p.missing.len(),
        p.renamed.len(),
        p.contacts.len()
    )).strong())
    .id_salt("import-preview")
    .default_open(true)
    .show(ui, |ui| {
        if !p.new.is_empty() {
            ui.label(RichText::new(format!("+ {} new clients", p.new.len())).strong());
            for (id, name) in p.new.iter().take(SHOWN) {
                ui.label(widgets::caption(format!("{name} ({id})")));
            }
            more(ui, p.new.len());
        }
        if !p.missing.is_empty() {
            ui.label(
                RichText::new(format!(
                    "− {} clients not in this CSV leave the client list (their folders, documents and contacts stay)",
                    p.missing.len()
                ))
                .strong()
                .color(theme::warn()),
            );
            for (id, name) in p.missing.iter().take(SHOWN) {
                ui.label(widgets::caption(format!("{name} ({id})")));
            }
            more(ui, p.missing.len());
        }
        if !p.renamed.is_empty() {
            ui.label(RichText::new(format!("{} renamed", p.renamed.len())).strong());
            for (id, old, new) in p.renamed.iter().take(SHOWN) {
                ui.label(widgets::caption(format!("{id}: {old} → {new}")));
            }
            more(ui, p.renamed.len());
        }
        if !p.contacts.is_empty() {
            ui.label(RichText::new(format!("@ {} contacts changed", p.contacts.len())).strong());
            for c in p.contacts.iter().take(SHOWN) {
                let who = if c.client.is_empty() {
                    c.id.clone()
                } else {
                    format!("{} {}", c.id, c.client)
                };
                let blank = |v: &str| if v.is_empty() { "(none)".to_string() } else { v.to_string() };
                let mut line = format!("{who} · {}: {} → {}", c.field, blank(&c.old), blank(&c.new));
                if c.kept_by_hand {
                    line.push_str(" (set by hand: the typed value is kept)");
                }
                ui.label(widgets::caption(line));
            }
            more(ui, p.contacts.len());
        }
    });
}

/// What the import needs, taken from the dialog.
struct Input {
    path: PathBuf,
    map: ColumnMap,
    known: BTreeMap<String, uuid::Uuid>,
    tin: Option<String>,
    contacts: ContactColumns,
}

fn run(
    d: &Input,
    conn: &mut vf_core::db::Connection,
    root: &std::path::Path,
    device_id: &str,
) -> Result<String, String> {
    let list = clients::import_csv_tin(root, &d.path, &d.map, &d.known, d.tin.as_deref())?;
    let contacts = vf_fs::contacts::set_columns(root, &d.contacts).and_then(|_| {
        if d.contacts.chosen().is_empty() {
            return Ok(None);
        }
        let table = read_csv(&d.path).map_err(|e| e.to_string())?;
        let rows = vf_fs::contacts::csv_rows(&table, &d.map, &d.contacts)?;
        vf_fs::contacts::store_from_csv(root, &list, &rows).map(Some)
    });
    // The same CSV for Intake: its re-imports read this file with these columns.
    let intake = clients::use_csv_in_intake(root, &d.path, &d.map).err();
    let rep = vf_fs::rebuild::sync_clients(conn, root, device_id, "clients_import")
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
    match intake {
        None => msg.push_str(" Intake › Setup uses this CSV too (each open Intake picks it up within a few seconds)."),
        Some(e) => msg.push_str(&format!(
            " Intake › Setup still names its own CSV ({e}): choose this one there too."
        )),
    }
    match contacts {
        Err(e) => msg.push_str(&format!(" The portal contacts were not saved: {e}")),
        Ok(Some(n)) => msg.push_str(&format!(
            " Portal contacts changed for {n} clients (typed ones kept): invite them from Admin › Portal › Invite from client list."
        )),
        Ok(None) => {}
    }
    match clients::unbound_matches(root, &list) {
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
