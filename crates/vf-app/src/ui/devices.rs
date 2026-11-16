//! Admin › Devices (PA-24): the workstations that have opened this cabinet (recorded in
//! `_Vibe/devices/` when each opens it), and taking an old one off the list.

use egui::RichText;

use crate::ui::browse::Env;
use crate::ui::widgets::UiExt as _;
use crate::ui::{theme, widgets};

/// Several Remote Desktop servers (PA-36): which one runs text recognition and B2 sync.
fn servers_section(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    use crate::server::Setting;
    ui.label(RichText::new("Servers").strong().size(16.0));
    // The choice for this computer (owner 2026-11-05): Windows decides, or always / never.
    let (now, auto) = *p.server.get_or_insert_with(|| {
        (
            crate::server::mode_setting(),
            crate::server::auto_detected(),
        )
    });
    let mut pick = now;
    ui.horizontal(|ui| {
        ui.label("Server mode on this computer");
        ui.radio_value(&mut pick, Setting::Auto, "Automatic")
            .on_hover_text(if auto {
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
                p.server = Some((pick, auto));
                p.notice = Some(
                    "Server mode changes when Vibe Files next starts on this computer.".into(),
                );
                p.error = None;
            }
            Err(e) => p.error = Some(format!("Could not save it: {e}")),
        }
    }
    let cloud = crate::cloudmode::is_cloud(env.root);
    if !crate::server::server_mode() && !cloud {
        ui.add_space(12.0);
        return;
    }
    ui.label(widgets::caption(if cloud && !crate::server::server_mode() {
        "The cabinet is in a cloud folder, so each PC keeps its own catalog of the cabinet and reads the other PCs' changes (when a client is opened and every 15 minutes). Text recognition and B2 sync run on one of them."
    } else {
        "Vibe Files runs from a network folder, so each server keeps its own catalog of the cabinet and reads the other servers' changes (when a client is opened and every 15 minutes). Text recognition and B2 sync run on one of them."
    }));
    let here = crate::server::computer();
    let main = crate::server::main_server(env.root);
    let machine = if crate::server::server_mode() {
        "server"
    } else {
        "PC"
    };
    ui.horizontal(|ui| {
        ui.label(match &main {
            Some(m) if m.computer.eq_ignore_ascii_case(&here) => {
                format!("Text recognition and B2 sync run on this {machine} ({here}).")
            }
            Some(m) => format!(
                "Text recognition and B2 sync run on {} (since {}).",
                m.computer,
                widgets::local_time(&m.since)
            ),
            None if cloud && !crate::server::server_mode() => "No PC runs text recognition and B2 sync yet: press Run them on this PC on the one that should.".into(),
            None => format!("No {machine} runs text recognition and B2 sync yet; the first to start Vibe Files takes it on."),
        });
        let is_here = main
            .as_ref()
            .is_some_and(|m| m.computer.eq_ignore_ascii_case(&here));
        if !is_here
            && ui
                .button(format!("Run them on this {machine}"))
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

/// Cabinets in OneDrive or Dropbox (PA-39): this computer's choice, and what it found.
fn cloud_section(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    use crate::cloudmode::Setting;
    ui.label(RichText::new("Cloud folders").strong().size(16.0));
    let now = *p.cloud.get_or_insert_with(crate::cloudmode::setting);
    let mut pick = now;
    let found = crate::cloudmode::detected(env.root);
    ui.horizontal(|ui| {
        ui.label("Cloud folder handling on this computer");
        ui.radio_value(&mut pick, Setting::Auto, "Automatic")
            .on_hover_text(match &found {
                Some(s) => format!("The cabinet is in a {s} folder: on."),
                None => "The cabinet is not in a OneDrive or Dropbox folder: off.".into(),
            });
        ui.radio_value(&mut pick, Setting::On, "Always on")
            .on_hover_text("For a folder Windows does not report, such as another sync service.");
        ui.radio_value(&mut pick, Setting::Off, "Always off")
            .on_hover_text(
                "Every file is read, online only or not: each is downloaded to this PC.",
            );
    });
    ui.label(widgets::caption(match crate::cloudmode::cabinet_provider(env.root) {
        Some(s) => format!(
            "Now on: the cabinet is in {s}. Files that are online only are not read in the background (indexing, text recognition, thumbnails, the weekly check) until they are on this PC; opening one downloads it. Each PC keeps its own catalog outside the cloud folder. A change takes effect when Vibe Files next starts."
        ),
        None if crate::cloudmode::running() == Setting::Off => "Now off: every file is read, online only or not (each is downloaded). A change takes effect when Vibe Files next starts.".into(),
        None => "The cabinet is not in a OneDrive or Dropbox folder. A file that is online only anyway is still left until it is on this PC. A change takes effect when Vibe Files next starts.".into(),
    }));
    if crate::server::server_mode() && found.is_some() {
        ui.label(
            RichText::new(
                "This is a Remote Desktop server and the cabinet is in a cloud folder: each person logged in syncs their own copy of it. Keep the cabinet on a file share for servers.",
            )
            .color(theme::error()),
        );
    }
    if pick != now {
        match crate::cloudmode::set_setting(pick) {
            Ok(()) => {
                p.cloud = Some(pick);
                p.notice = Some(
                    "Cloud folder handling changes when Vibe Files next starts on this computer."
                        .into(),
                );
                p.error = None;
            }
            Err(e) => p.error = Some(format!("Could not save it: {e}")),
        }
    }
    ui.add_space(12.0);
}

/// This PC's shared catalog (Q-67: a Remote Desktop server's logins index the cabinet once).
fn shared_section(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    ui.label(RichText::new("This PC's catalog").strong().size(16.0));
    let Ok(cab) = vf_fs::cabinet::read(env.root) else {
        return;
    };
    ui.label(widgets::caption(crate::catalog_place::describe()));
    catalog_facts(ui, p, env);
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
                "On a Remote Desktop server where several people use this cabinet, share this login's catalog: the others then use it instead of each reading the whole cabinet. It is kept where the line above says.",
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

/// What the catalog in use holds, for Admin (PA-86): read on a thread, shown until Refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub file: std::path::PathBuf,
    /// Size on disk, and when the file was last written (local time), when the file says.
    pub bytes: Option<u64>,
    pub written: Option<String>,
    pub documents: i64,
    pub clients: i64,
    pub clients_with_folder: i64,
    /// An index is asked for or under way.
    pub index_due: bool,
}

/// [`Facts`] of the catalog at `file`, through a connection of its own.
pub fn facts(file: &std::path::Path) -> Result<Facts, String> {
    // Read only, with no setting changed: this is a look at the catalog in use, wherever
    // and however it is kept.
    let conn = vf_core::db::Connection::open_with_flags(
        file,
        vf_core::db::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(10));
    // A count that cannot be read is an error, never a 0 (a healthy catalog must not read
    // as empty while the index writes).
    let n = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, [], |r| r.get(0))
            .map_err(|e| format!("the catalog is busy or cannot be read ({e})"))
    };
    // The catalog's file and its write-ahead log beside it (what was written since the log
    // was last folded into the file is there): the size of both, the later of their times.
    let wal = std::path::PathBuf::from(format!("{}-wal", file.display()));
    let metas: Vec<std::fs::Metadata> = [file, wal.as_path()]
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .collect();
    Ok(Facts {
        file: file.to_path_buf(),
        bytes: (!metas.is_empty()).then(|| metas.iter().map(|m| m.len()).sum()),
        written: metas
            .iter()
            .filter_map(|m| m.modified().ok())
            .max()
            .map(|t| crate::ui::explorer::modified_text(Some(t))),
        documents: n("SELECT count(*) FROM document WHERE deleted_at IS NULL")?,
        clients: n("SELECT count(*) FROM client WHERE deleted_at IS NULL")?,
        clients_with_folder: n(
            "SELECT count(*) FROM client WHERE deleted_at IS NULL AND coalesce(folder_rel, '') <> ''",
        )?,
        index_due: vf_fs::rebuild::unfinished_label(&conn).is_some(),
    })
}

/// The catalog's facts and "Index the cabinet again" (PA-86, owner 2026-11-16: the catalog's
/// folder cannot be opened from their login; what it holds is shown here, and it can be put
/// right without reaching the file).
fn catalog_facts(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    if let Some(r) = crate::ui::job::take(&mut p.facts_job) {
        p.facts = Some(r.and_then(|f| f));
    }
    if p.facts.is_none() && p.facts_job.is_none() {
        // The file the open connection is on (the catalog in use, wherever it is kept).
        let file: Option<String> = env
            .conn
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name = 'main'",
                [],
                |r| r.get(0),
            )
            .ok()
            .filter(|f: &String| !f.is_empty());
        match file {
            Some(file) => {
                match crate::ui::job::Job::start(ui.ctx(), "Reading the catalog…", move || {
                    facts(std::path::Path::new(&file))
                }) {
                    Ok(j) => p.facts_job = Some(j),
                    Err(e) => p.facts = Some(Err(e)),
                }
            }
            None => p.facts = Some(Err("the catalog's file is not known".into())),
        }
    }
    if let Some(j) = &p.facts_job {
        j.show(ui);
        return;
    }
    match p.facts.clone() {
        Some(Ok(f)) => {
            ui.label(format!("In use: {}", f.file.display()));
            let size = f.bytes.map_or_else(
                || "size not known".to_string(),
                |b| format!("{:.1} MB", b as f64 / 1_048_576.0),
            );
            ui.label(format!(
                "{} documents · {} of {} clients have a folder · {size}{}",
                f.documents,
                f.clients_with_folder,
                f.clients,
                f.written
                    .as_deref()
                    .map_or(String::new(), |w| format!(" · last written {w}"))
            ));
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() {
                    p.facts = None;
                }
                if f.index_due {
                    ui.label(widgets::caption(
                        "The cabinet is being indexed, or will be when Vibe Files next opens it on this computer.",
                    ));
                } else if !p.ask_index
                    && ui
                        .button("Index the cabinet again…")
                        .on_hover_text(
                            "For a catalog that is missing clients' folders or documents.",
                        )
                        .clicked()
                {
                    p.ask_index = true;
                }
            });
            // Asked first (it changes the catalog for every login of this computer).
            if p.ask_index && !f.index_due {
                ui.label(
                    "The next time this cabinet is opened on this computer, by anyone, its folders are listed again and the catalog brought in line: files it does not have are added, clients get the folders bound to them, and documents whose files are gone are taken out. No file is read or changed. In the Vibe Files window it runs behind the window; a command-line run waits for it.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Index at the next open").clicked() {
                        p.ask_index = false;
                        match vf_fs::rebuild::ask_for_index(env.conn) {
                            Ok(()) => {
                                p.notice = Some(
                                    "Close Vibe Files and open it again on this computer: the cabinet is indexed again."
                                        .into(),
                                );
                                p.error = None;
                                p.facts = None;
                            }
                            Err(e) => p.error = Some(format!("Not asked for: {e}")),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        p.ask_index = false;
                    }
                });
            }
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("The catalog was not read: {e}")).color(theme::error()));
            if ui.button("Refresh").clicked() {
                p.facts = None;
            }
        }
        None => {}
    }
    ui.add_space(6.0);
}

#[derive(Default)]
pub struct DevicesPage {
    pub notice: Option<String>,
    pub error: Option<String>,
    /// The catalog's facts (PA-86), and their reading.
    facts: Option<Result<Facts, String>>,
    facts_job: Option<crate::ui::job::Job<Result<Facts, String>>>,
    /// "Index the cabinet again…" was pressed: what it does is shown, to be confirmed.
    ask_index: bool,
    /// This computer's server-mode choice and what Windows says, read once (not every frame).
    server: Option<(crate::server::Setting, bool)>,
    /// This computer's cloud folder choice, read once (PA-39).
    cloud: Option<crate::cloudmode::Setting>,
}

pub fn show(ui: &mut egui::Ui, p: &mut DevicesPage, env: &mut Env) {
    shared_section(ui, p, env);
    ui.add_space(12.0);
    ui.add_space(12.0);
    servers_section(ui, p, env);
    cloud_section(ui, p, env);
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
                    .small_btn("Remove")
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
