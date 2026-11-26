//! Organizer links (PA-113, owner 2026-11-25): Admin › Portal › Organizers lists each client's
//! link to the firm's online organizer (Stanford Tax) for a tax year, with Turn off and Turn on;
//! Import CSV… reads the organizer app's export, matches each row to a client (its UltraTax ID,
//! an email on file with the name, or the name alone as a suggestion), shows the preview, and
//! sends the ticked rows to the portal. The portal shows an Organizer card on the client's home
//! page until the tax return delivery for that year is created or the card is turned off.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::PortalSettings;
use vf_portal::organizer::{self, By, Candidate, Matched, Organizer, Status};

use crate::portal as core;
use crate::ui::portal::{AuditOut, ClientRef, spawn};
use crate::ui::{theme, widgets};

type Clients = HashMap<String, (ClientRef, Option<String>)>;
/// The export's rows and the cabinet's clients with their Portal contacts.
type Read = (
    Vec<organizer::ExportRow>,
    Vec<vf_fs::contacts::ClientContacts>,
);

/// Which links the page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Show {
    All,
    Showing,
    Off,
}

pub struct OrganizersPage {
    year: i32,
    show: Show,
    data: Option<Vec<Organizer>>,
    clients: Clients,
    job: Option<Receiver<Result<Vec<Organizer>, String>>>,
    act: Option<Receiver<Result<String, String>>>,
    loaded: bool,
    ticked: BTreeSet<String>,
    msg: Option<String>,
    err: Option<String>,
    import: Option<ImportWindow>,
    settings: Option<(std::time::Instant, Option<PortalSettings>)>,
}

impl Default for OrganizersPage {
    fn default() -> Self {
        Self {
            year: chrono::Datelike::year(&chrono::Local::now()) - 1,
            show: Show::All,
            data: None,
            clients: HashMap::new(),
            job: None,
            act: None,
            loaded: false,
            ticked: BTreeSet::new(),
            msg: None,
            err: None,
            import: None,
            settings: None,
        }
    }
}

impl OrganizersPage {
    /// Read again when the page is opened.
    pub fn reread(&mut self) {
        self.loaded = false;
    }

    /// Opens the import preview of the export at `path` (Import CSV…'s file, or a test's).
    pub(crate) fn start_import(
        &mut self,
        ctx: &egui::Context,
        root: PathBuf,
        path: PathBuf,
        known: Vec<ClientRef>,
    ) {
        self.import = Some(ImportWindow::new(ctx, root, path, known));
    }

    /// Ticks or unticks the preview's row of the export's `line` (a test stands in for the click).
    #[cfg(test)]
    pub(crate) fn set_import_row(&mut self, line: usize, ticked: bool) {
        if let Some(w) = &mut self.import
            && let Some(m) = w.rows.iter_mut().find(|m| m.row.line == line)
        {
            m.ticked = ticked;
        }
    }

    /// Ticks every row shown (a test stands in for the clicks).
    #[cfg(test)]
    pub(crate) fn tick_shown(&mut self) {
        for o in self.data.iter().flatten() {
            let shown = match self.show {
                Show::All => true,
                Show::Showing => o.off.is_none(),
                Show::Off => o.off.is_some(),
            };
            if shown {
                self.ticked.insert(o.id.clone());
            }
        }
    }
}

fn client_rows(env: &mut crate::ui::browse::Env) -> Clients {
    let Ok(mut stmt) = env
        .conn
        .prepare("SELECT id, ext_id, name, folder_rel FROM client WHERE deleted_at IS NULL")
    else {
        return HashMap::new();
    };
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            (
                ClientRef {
                    id: r.get(0)?,
                    code: r.get(1)?,
                    name: r.get(2)?,
                },
                r.get::<_, Option<String>>(3)?,
            ),
        ))
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

fn audit_row(audit: &Option<AuditOut>, action: &str, detail: serde_json::Value) {
    if let Some(a) = audit {
        a.send(AuditEntry::new(action, None, detail));
    }
}

pub fn page(
    ui: &mut egui::Ui,
    page: &mut OrganizersPage,
    env: &mut crate::ui::browse::Env,
    audit: Option<AuditOut>,
) {
    ui.heading("Organizers");
    if page
        .settings
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() >= std::time::Duration::from_secs(30))
    {
        let s = vf_fs::cabinet::read(env.root)
            .ok()
            .and_then(|c| vf_portal::settings::load(env.root, &c.cabinet_id));
        page.settings = Some((std::time::Instant::now(), s));
    }
    let Some(s) = page.settings.as_ref().and_then(|(_, s)| s.clone()) else {
        ui.label("The portal is not set up for this cabinet. Set it up in Admin › Portal.");
        return;
    };
    ui.label(widgets::caption(
        "Each client's link to the firm's online organizer, from the organizer app's export. \
         The client's portal shows an Organizer card until the tax return delivery for that \
         year is created, or the card is turned off here.",
    ));
    let ctx = ui.ctx().clone();
    if let Some(rx) = &page.job
        && let Ok(r) = rx.try_recv()
    {
        page.job = None;
        match r {
            Ok(d) => {
                page.data = Some(d);
                page.err = None;
            }
            Err(e) => page.err = Some(e),
        }
    }
    if let Some(rx) = &page.act
        && let Ok(r) = rx.try_recv()
    {
        page.act = None;
        match r {
            Ok(m) => {
                page.msg = Some(m);
                page.ticked.clear();
                page.loaded = false;
            }
            Err(e) => page.err = Some(e),
        }
    }
    if !page.loaded {
        page.loaded = true;
        page.clients = client_rows(env);
        let (s2, year) = (s.clone(), page.year);
        page.job = Some(spawn(&ctx, move || {
            core::client(&s2).organizers(Some(year), None).map_err(|e| {
                if e.status() == Some(404) {
                    "This portal has no organizer links yet: update the portal (0.5.5 or later)."
                        .to_string()
                } else {
                    e.message()
                }
            })
        }));
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Tax year");
        let before = page.year;
        ui.add(egui::DragValue::new(&mut page.year).range(2000..=2100));
        if page.year != before {
            page.ticked.clear();
            page.loaded = false;
        }
        ui.separator();
        ui.label("Show:");
        ui.selectable_value(&mut page.show, Show::All, "All");
        ui.selectable_value(&mut page.show, Show::Showing, "Card showing");
        ui.selectable_value(&mut page.show, Show::Off, "Off");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(page.job.is_none(), egui::Button::new("Read again"))
                .clicked()
            {
                page.loaded = false;
            }
            if ui
                .add_enabled(page.import.is_none(), egui::Button::new("Import CSV…"))
                .on_hover_text("The organizer app's export: each client's questionnaire link")
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .set_title("The organizer app's export")
                    .pick_file()
            {
                let known = page.clients.values().map(|(c, _)| c.clone()).collect();
                page.start_import(&ctx, env.root.to_path_buf(), path, known);
            }
            if page.job.is_some() || page.act.is_some() {
                ui.spinner();
            }
        });
    });
    if let Some(m) = &page.msg {
        ui.label(RichText::new(m).color(theme::ink_secondary()));
    }
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    if let Some(list) = &page.data {
        let rows: Vec<&Organizer> = list
            .iter()
            .filter(|o| match page.show {
                Show::All => true,
                Show::Showing => o.off.is_none(),
                Show::Off => o.off.is_some(),
            })
            .collect();
        let showing = list.iter().filter(|o| o.off.is_none()).count();
        let by_delivery = list
            .iter()
            .filter(|o| o.off.as_deref() == Some("delivery"))
            .count();
        let turned = list
            .iter()
            .filter(|o| o.off.as_deref() == Some("off"))
            .count();
        ui.label(widgets::caption(format!(
            "{} listed · {showing} showing · {by_delivery} off by delivery · {turned} turned off",
            rows.len()
        )));
        if rows.is_empty() {
            ui.label(widgets::caption(if list.is_empty() {
                "None for this tax year. Import the organizer app's export with Import CSV…"
            } else {
                "None."
            }));
        }
        let name_of = |id: &str| {
            page.clients
                .get(id)
                .map(|(c, _)| format!("{} ({})", c.name, c.code))
                .unwrap_or_else(|| "(not in this cabinet)".into())
        };
        egui::Grid::new("organizers-list")
            .num_columns(5)
            .striped(true)
            .spacing([14.0, 6.0])
            .show(ui, |ui| {
                ui.label("");
                for h in ["Client", "Type", "Organizer", "Card"] {
                    ui.label(RichText::new(h).small().color(theme::ink_muted()));
                }
                ui.end_row();
                for o in rows {
                    let mut t = page.ticked.contains(&o.id);
                    if ui.checkbox(&mut t, "").changed() {
                        if t {
                            page.ticked.insert(o.id.clone());
                        } else {
                            page.ticked.remove(&o.id);
                        }
                    }
                    ui.label(name_of(&o.client_id));
                    ui.label(&o.return_type);
                    ui.label(o.status.words());
                    ui.label(o.card_words());
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let n = page.ticked.len();
            let idle = page.act.is_none() && n > 0;
            for (off, words) in [(true, "Turn off ticked"), (false, "Turn on ticked")] {
                if ui.add_enabled(idle, egui::Button::new(words)).clicked() {
                    let ids: Vec<String> = page.ticked.iter().cloned().collect();
                    let (s2, year, a) = (s.clone(), page.year, audit.clone());
                    page.msg = None;
                    page.err = None;
                    page.act = Some(spawn(&ctx, move || {
                        let changed = core::client(&s2)
                            .set_organizers_off(&ids, off)
                            .map_err(|e| e.message())?;
                        audit_row(
                            &a,
                            if off {
                                "portal_organizer_off"
                            } else {
                                "portal_organizer_on"
                            },
                            serde_json::json!({ "taxYear": year, "count": changed, "organizer_ids": ids }),
                        );
                        let mut m = format!(
                            "{changed} card{} turned {}.",
                            filer_intake::ui::theme::plural(changed as usize),
                            if off { "off" } else { "on" }
                        );
                        if (changed as usize) < ids.len() {
                            m.push_str(if off {
                                " The others were off already."
                            } else {
                                " The others were not turned off by hand: a card off because the \
                                 delivery started, or delivered, stays off."
                            });
                        }
                        Ok(m)
                    }));
                }
            }
            ui.label(widgets::caption(format!("{n} ticked")));
        });
    }
    if let Some(w) = &mut page.import {
        w.show(&ctx, &s, &page.clients, audit.clone());
        if w.closed {
            if w.sent {
                page.year = w.year;
                page.loaded = false;
            }
            page.import = None;
        }
    }
}

/// The import preview: each row of the export with the client it goes to.
pub struct ImportWindow {
    path: PathBuf,
    year: i32,
    rows: Vec<Matched>,
    /// The client chosen for each row.
    choice: Vec<Option<String>>,
    find: Vec<String>,
    job: Option<Receiver<Result<Read, String>>>,
    dir: Vec<vf_fs::contacts::ClientContacts>,
    exported: Vec<organizer::ExportRow>,
    send: Option<Receiver<Result<String, String>>>,
    msg: Option<String>,
    err: Option<String>,
    /// Tell the clients' people their organizer is ready (owner Q&A 2026-11-25: once).
    tell: bool,
    pub closed: bool,
    pub sent: bool,
}

impl ImportWindow {
    fn new(ctx: &egui::Context, root: PathBuf, path: PathBuf, known: Vec<ClientRef>) -> Self {
        let p = path.clone();
        Self {
            path,
            year: chrono::Datelike::year(&chrono::Local::now()) - 1,
            rows: Vec::new(),
            choice: Vec::new(),
            find: Vec::new(),
            job: Some(spawn(ctx, move || {
                let rows = organizer::read_export(&p)?;
                // No client CSV imported yet: the catalog's clients, by ID and name only.
                let dir = vf_fs::contacts::directory(&root).unwrap_or_else(|_| {
                    known
                        .into_iter()
                        .map(|c| vf_fs::contacts::ClientContacts {
                            client_id: c.id,
                            code: c.code,
                            name: c.name,
                            partner: None,
                            planned: Default::default(),
                            names: Vec::new(),
                        })
                        .collect()
                });
                Ok((rows, dir))
            })),
            dir: Vec::new(),
            exported: Vec::new(),
            send: None,
            msg: None,
            err: None,
            tell: true,
            closed: false,
            sent: false,
        }
    }

    fn rematch(&mut self) {
        self.rows = organizer::match_rows(&self.exported, &self.dir, self.year);
        self.choice = self
            .rows
            .iter()
            .map(|m| m.client.as_ref().map(|c| c.client_id.clone()))
            .collect();
        self.find = vec![String::new(); self.rows.len()];
    }

    /// Matched again for another tax year; a client staff chose and a tick they set stay where
    /// the row could be sent before (QA 2026-11-25: changing the year lost them).
    fn rematch_keeping_picks(&mut self) {
        let before: Vec<(bool, Option<String>, bool)> = self
            .rows
            .iter()
            .zip(&self.choice)
            .map(|(m, c)| (m.refused, c.clone(), m.ticked))
            .collect();
        self.rematch();
        for (i, (refused, choice, ticked)) in before.into_iter().enumerate() {
            if refused || i >= self.rows.len() || self.rows[i].refused {
                continue;
            }
            self.choice[i] = choice;
            self.rows[i].ticked = ticked;
        }
    }

    fn show(
        &mut self,
        ctx: &egui::Context,
        s: &PortalSettings,
        clients: &Clients,
        audit: Option<AuditOut>,
    ) {
        if let Some(rx) = &self.job
            && let Ok(r) = rx.try_recv()
        {
            self.job = None;
            match r {
                Ok((rows, dir)) => {
                    // The year most rows are for.
                    let mut years: HashMap<i32, usize> = HashMap::new();
                    for y in rows.iter().filter_map(|r| r.tax_year) {
                        *years.entry(y).or_default() += 1;
                    }
                    if let Some((y, _)) = years.into_iter().max_by_key(|(y, n)| (*n, *y)) {
                        self.year = y;
                    }
                    self.exported = rows;
                    self.dir = dir;
                    self.rematch();
                }
                Err(e) => self.err = Some(e),
            }
        }
        if let Some(rx) = &self.send
            && let Ok(r) = rx.try_recv()
        {
            self.send = None;
            match r {
                Ok(m) => {
                    self.msg = Some(m);
                    self.sent = true;
                }
                Err(e) => self.err = Some(e),
            }
        }
        let mut open = true;
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        egui::Window::new(format!("Import organizer links: {name}"))
            .open(&mut open)
            .default_size([900.0, 620.0])
            .collapsible(false)
            .show(ctx, |ui| {
                if self.job.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the export and the client list…");
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label("Tax year");
                    let before = self.year;
                    ui.add(egui::DragValue::new(&mut self.year).range(2000..=2100));
                    if self.year != before {
                        self.rematch_keeping_picks();
                    }
                });
                let count = |b: By| self.rows.iter().filter(|m| m.by == b && !m.refused).count();
                ui.label(format!(
                    "{} rows · by ID {} · by email and name {} · check {} · name only {} · no match {} · not taken {}",
                    self.rows.len(),
                    count(By::Id),
                    count(By::EmailAndName),
                    count(By::Check),
                    count(By::Name),
                    count(By::None),
                    self.rows.iter().filter(|m| m.refused).count(),
                ));
                ui.label(widgets::caption(format!(
                    "Only links on {} are taken. A row that asks, or a name-only suggestion, is sent \
                     only when ticked. A card turned off stays off.",
                    organizer::HOSTS.join(", ")
                )));
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(ui.available_height() - 70.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("organizer-import")
                            .num_columns(5)
                            .striped(true)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.label("");
                                for h in ["In the export", "Goes to", "How", "Organizer"] {
                                    ui.label(RichText::new(h).small().color(theme::ink_muted()));
                                }
                                ui.end_row();
                                for i in 0..self.rows.len() {
                                    self.row(ui, i, clients);
                                    ui.end_row();
                                }
                            });
                    });
                ui.separator();
                let ready: Vec<(String, organizer::ExportRow)> = self
                    .rows
                    .iter()
                    .zip(&self.choice)
                    .filter(|(m, c)| m.ticked && !m.refused && c.is_some())
                    .map(|(m, c)| (c.clone().unwrap_or_default(), m.row.clone()))
                    .collect();
                // One link per client and year: two rows going to one client are refused.
                let mut seen: HashMap<&str, usize> = HashMap::new();
                for (c, _) in &ready {
                    *seen.entry(c.as_str()).or_default() += 1;
                }
                let twice: Vec<String> = seen
                    .iter()
                    .filter(|(_, n)| **n > 1)
                    .map(|(id, _)| {
                        clients
                            .get(*id)
                            .map(|(c, _)| format!("{} ({})", c.name, c.code))
                            .unwrap_or_else(|| (*id).to_string())
                    })
                    .collect();
                if !twice.is_empty() {
                    ui.colored_label(
                        theme::error(),
                        format!(
                            "Two ticked rows go to {}: a client has one organizer link a year. Untick one.",
                            twice.join(", ")
                        ),
                    );
                }
                ui.checkbox(
                    &mut self.tell,
                    "Tell the clients' people by email or text that their organizer is ready (once)",
                )
                .on_hover_text(
                    "Each new link: everyone with portal access at the client, the way they chose; \
                     the message has the portal's address, not the organizer link",
                );
                ui.horizontal(|ui| {
                    let n = ready.len();
                    if ui
                        .add_enabled(
                            n > 0 && twice.is_empty() && self.send.is_none() && !self.sent,
                            egui::Button::new(format!(
                                "Send {n} link{}",
                                filer_intake::ui::theme::plural(n)
                            )),
                        )
                        .clicked()
                    {
                        self.err = None;
                        let (s2, year, a, tell) = (s.clone(), self.year, audit.clone(), self.tell);
                        let names: HashMap<String, ClientRef> =
                            clients.iter().map(|(k, (c, _))| (k.clone(), c.clone())).collect();
                        self.send =
                            Some(spawn(ctx, move || send(&s2, year, ready, &names, &a, tell)));
                    }
                    if self.send.is_some() {
                        ui.spinner();
                    }
                    if ui.button("Close").clicked() {
                        self.closed = true;
                    }
                });
                if let Some(m) = &self.msg {
                    ui.label(RichText::new(m).color(theme::ink_secondary()));
                }
                if let Some(e) = &self.err {
                    ui.colored_label(theme::error(), e);
                }
            });
        if !open {
            self.closed = true;
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, i: usize, clients: &Clients) {
        let m = &mut self.rows[i];
        ui.add_enabled_ui(!m.refused && self.choice[i].is_some(), |ui| {
            ui.checkbox(&mut m.ticked, "");
        });
        ui.label(&m.row.client);
        let label = |id: &str| {
            clients
                .get(id)
                .map(|(c, _)| format!("{} ({})", c.name, c.code))
                .unwrap_or_else(|| id.to_string())
        };
        ui.vertical(|ui| {
            if m.refused {
                ui.label("—");
                return;
            }
            let shown = self.choice[i]
                .as_deref()
                .map(label)
                .unwrap_or_else(|| "Pick a client…".into());
            let before = self.choice[i].clone();
            egui::ComboBox::from_id_salt(("organizer-client", i))
                .selected_text(shown)
                .width(260.0)
                .show_ui(ui, |ui| {
                    for c in &m.candidates {
                        ui.selectable_value(
                            &mut self.choice[i],
                            Some(c.client_id.clone()),
                            c.label(),
                        );
                    }
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.find[i])
                            .hint_text("Find a client by name or ID")
                            .desired_width(240.0),
                    );
                    let f = self.find[i].trim().to_lowercase();
                    if f.len() >= 2 {
                        let mut hits: Vec<&Candidate> = Vec::new();
                        let all: Vec<Candidate> = self
                            .dir
                            .iter()
                            .filter(|c| {
                                c.name.to_lowercase().contains(&f) || c.code.to_lowercase() == f
                            })
                            .take(30)
                            .map(|c| Candidate {
                                client_id: c.client_id.clone(),
                                code: c.code.clone(),
                                name: c.name.clone(),
                            })
                            .collect();
                        hits.extend(all.iter());
                        for c in hits {
                            ui.selectable_value(
                                &mut self.choice[i],
                                Some(c.client_id.clone()),
                                c.label(),
                            );
                        }
                    }
                });
            // Choosing a client by hand ticks the row.
            if self.choice[i] != before && self.choice[i].is_some() {
                m.ticked = true;
            }
            if !m.note.is_empty() {
                ui.label(widgets::caption(&m.note));
            }
        });
        ui.label(match m.by {
            _ if m.refused => m.note.clone(),
            By::Id => "UltraTax ID".into(),
            By::EmailAndName => "Email and name".into(),
            By::Check => "Check".into(),
            By::Name => "Name only".into(),
            By::None => "No match".into(),
        });
        ui.label(m.row.status.words());
    }
}

/// Sends the ticked rows; a client not on the portal yet is added first (owner Q&A: the card
/// waits there for whoever signs in).
fn send(
    s: &PortalSettings,
    year: i32,
    rows: Vec<(String, organizer::ExportRow)>,
    names: &HashMap<String, ClientRef>,
    audit: &Option<AuditOut>,
    tell: bool,
) -> Result<String, String> {
    let p = core::client(s);
    let refs: Vec<(String, &organizer::ExportRow)> =
        rows.iter().map(|(c, r)| (c.clone(), r)).collect();
    let mut r = p
        .put_organizers(year, &refs, tell)
        .map_err(|e| e.message())?;
    let missing: Vec<&String> = r
        .refused
        .iter()
        .filter(|x| x.why == "client_not_found")
        .map(|x| &x.client_id)
        .collect();
    if !missing.is_empty() {
        let mut added = Vec::new();
        for id in missing {
            if let Some(c) = names.get(id)
                && p.put_client(&c.id, &c.code, &c.name, None).is_ok()
            {
                added.push(id.clone());
            }
        }
        let again: Vec<(String, &organizer::ExportRow)> = refs
            .iter()
            .filter(|(c, _)| added.contains(c))
            .cloned()
            .collect();
        if !again.is_empty() {
            let r2 = p
                .put_organizers(year, &again, tell)
                .map_err(|e| e.message())?;
            r.added += r2.added;
            r.told += r2.told;
            r.nobody += r2.nobody;
            r.changed += r2.changed;
            r.same += r2.same;
            r.refused.retain(|x| !added.contains(&x.client_id));
            r.refused.extend(r2.refused);
        }
    }
    audit_row(
        audit,
        "portal_organizers_import",
        serde_json::json!({
            "taxYear": year,
            "sent": rows.len(),
            "added": r.added,
            "changed": r.changed,
            "same": r.same,
            "told": r.told,
            "refused": r.refused.len(),
        }),
    );
    let mut m = format!(
        "Sent: {} new, {} changed, {} the same.",
        r.added, r.changed, r.same
    );
    if tell && r.added > 0 {
        m.push_str(&format!(
            " {} {} told their organizer is ready",
            r.told,
            if r.told == 1 { "person" } else { "people" }
        ));
        if r.nobody > 0 {
            m.push_str(&format!(
                "; {} client{} nobody on the portal yet (the card waits for them)",
                r.nobody,
                if r.nobody == 1 { " has" } else { "s have" }
            ));
        }
        m.push('.');
    }
    if !r.refused.is_empty() {
        m.push_str(&format!(" {} not taken by the portal.", r.refused.len()));
    }
    Ok(m)
}

/// The client panel's organizer line for `client_id`: the newest tax year's.
pub fn panel_organizer<'a>(
    read: Option<&'a Vec<Organizer>>,
    client_id: &str,
) -> Option<&'a Organizer> {
    read?
        .iter()
        .filter(|o| o.client_id == client_id)
        .max_by_key(|o| o.tax_year)
}

/// "Organizer 2025: In progress · card showing".
pub fn panel_words(o: &Organizer) -> String {
    let card = match o.off.as_deref() {
        None => "card showing".to_string(),
        Some("delivery") => "card off (delivery started)".into(),
        Some("delivered") => "card off (delivered)".into(),
        Some(_) => "card turned off".into(),
    };
    let status = if o.status == Status::Unknown {
        ""
    } else {
        o.status.words()
    };
    format!(
        "Organizer {}: {}{}{card}",
        o.tax_year,
        status,
        if status.is_empty() { "" } else { " · " }
    )
}
