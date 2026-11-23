//! To-do lists (PA-103; Vibe-Portal P109): a client's lists from the client panel's To-do
//! lists…, and Admin › To-do templates. Staff make, read, change and delete them here; the
//! client answers on the portal. Every portal call runs on a thread with "Working…" shown; the
//! window updates in place from what the call answered (no re-read of everything after each
//! change). Each list read is copied into the client's folder (`_Vibe/todo/<id>.jsonl`) through
//! the app's portal state, which records it with an audit row; with the portal out of reach the
//! window shows those copies.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::PortalSettings;
use vf_portal::todo::{
    self, CopyLine, ItemIn, ListChange, NewList, StaffChoice, Template, TodoList,
};

use crate::portal as core;
use crate::ui::portal::{AuditOut, ClientRef, spawn};
use crate::ui::widgets::UiExt as _;
use crate::ui::{theme, widgets};

/// What a change did ("The list was sent."), in the app's OK colour: the window's errors have
/// their own red line (walkthrough 2026-11-23: success showed in red).
fn done_line(ui: &mut egui::Ui, msg: Option<&str>) {
    ui.add_sized(
        [ui.available_width(), 36.0],
        egui::Label::new(RichText::new(msg.unwrap_or("")).color(theme::health_ok())).wrap(),
    );
}

/// The answer of a portal read run on a scoped thread (a panic there is a failed read).
fn joined<T>(
    h: std::thread::ScopedJoinHandle<'_, vf_portal::api::Result<T>>,
) -> vf_portal::api::Result<T> {
    h.join()
        .unwrap_or_else(|_| Err(vf_portal::PortalError::Local("a read stopped".into())))
}

/// A list as read, to be appended to its copy in the client folder (when it changed).
#[derive(Debug, Clone)]
pub struct TodoCopy {
    pub path: PathBuf,
    pub line: String,
    pub list_id: String,
    pub client_id: String,
    pub title: String,
}

/// Writes the copies of lists read into their client folders, in one Transaction with an audit
/// row for each (CLAUDE.md principle 5). Append-only: a new line, never a rewrite.
pub fn write_copies(
    conn: &mut vf_core::db::Connection,
    root: &std::path::Path,
    device_id: &str,
    copies: Vec<TodoCopy>,
) -> Result<(), String> {
    if copies.is_empty() {
        return Ok(());
    }
    let mut tx = vf_core::tx::Transaction::new("portal_todo_copy").record_only();
    for c in copies {
        let rel = c
            .path
            .strip_prefix(root)
            .unwrap_or(&c.path)
            .to_string_lossy()
            .replace('\\', "/");
        tx = tx
            .file(vf_core::tx::FileOp::AppendLines {
                path: c.path,
                lines: vec![c.line],
                undo_lines: Vec::new(),
            })
            .audit(AuditEntry::new(
                "portal_todo_copy",
                None,
                serde_json::json!({
                    "client_id": c.client_id,
                    "list_id": c.list_id,
                    "title": c.title,
                    "copy": rel,
                }),
            ));
    }
    let mut fs = vf_fs::fileops::FilerFileOps::new(&[root]).map_err(|e| e.to_string())?;
    tx.commit(conn, &mut fs, &vf_core::tx::Actor::device(device_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Where the window's threads send the copies to write (the app's portal state).
#[derive(Clone)]
pub struct CopyOut(pub(crate) Sender<TodoCopy>);

/// Where a list's copy goes, and the line to add when it changed.
fn copy_of(
    root: &std::path::Path,
    folder: Option<&str>,
    l: &TodoList,
    by: &str,
) -> Option<TodoCopy> {
    let folder = folder?;
    // Only into the folder still bound to this client: a folder renamed or moved since the
    // window opened is not made again as an unbound one (QA 2026-11-23).
    let bound = filer_intake::sentinel::read_sentinel(&root.join(folder))
        .ok()
        .flatten()
        .map(|s| s.client_id);
    if bound.is_none() || bound != uuid::Uuid::parse_str(&l.client_id).ok() {
        return None;
    }
    let path = todo::copy_path(root, folder, &l.id);
    let last = todo::newest_copy(&path);
    let line = todo::copy_line(last.as_ref(), l, &vf_core::now_rfc3339(), by)?;
    Some(TodoCopy {
        path,
        line,
        list_id: l.id.clone(),
        client_id: l.client_id.clone(),
        title: l.title.clone(),
    })
}

fn days_words(n: u32) -> String {
    match n {
        0 => "Never".into(),
        n => format!("Every {n} days while items are open"),
    }
}

const REMIND_CHOICES: [u32; 4] = [0, 3, 7, 14];

/// "2027-03-15" or empty; anything else is refused.
fn due_ok(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn kind_words(kind: &str) -> &'static str {
    if kind == todo::KIND_TEXT {
        "An answer"
    } else {
        "Documents"
    }
}

fn state_words(state: &str) -> &'static str {
    match state {
        "answered" => "Answered",
        "not_applicable" => "Does not apply",
        _ => "Waiting",
    }
}

/// The item editor: kind, what to ask, required, doc type; move, remove, add. Items the client
/// answered (`locked`, by id) keep their wording and kind.
fn items_editor(
    ui: &mut egui::Ui,
    id: &str,
    items: &mut Vec<ItemIn>,
    locked: &HashSet<String>,
    doc_types: &[String],
) {
    let mut remove: Option<usize> = None;
    let mut up: Option<usize> = None;
    let n = items.len();
    egui::Grid::new(format!("todo-items-{id}"))
        .num_columns(5)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(widgets::caption("Asks for"));
            ui.label(widgets::caption("What to ask"));
            ui.label(widgets::caption("Required"));
            ui.label(widgets::caption("Doc type"));
            ui.label("");
            ui.end_row();
            for (i, it) in items.iter_mut().enumerate() {
                let fixed = it.id.as_ref().is_some_and(|x| locked.contains(x));
                ui.add_enabled_ui(!fixed, |ui| {
                    egui::ComboBox::from_id_salt(format!("todo-kind-{id}-{i}"))
                        .selected_text(kind_words(&it.kind))
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut it.kind,
                                todo::KIND_DOCUMENT.into(),
                                "Documents",
                            );
                            ui.selectable_value(&mut it.kind, todo::KIND_TEXT.into(), "An answer");
                        });
                });
                // A fixed size: inside the grid a text box ignores its desired width.
                ui.add_enabled_ui(!fixed, |ui| {
                    ui.add_sized(
                        [320.0, 22.0],
                        egui::TextEdit::singleline(&mut it.prompt)
                            .hint_text("W-2 from each employer")
                            .char_limit(500),
                    )
                    .on_disabled_hover_text(
                        "The client has answered this item: its wording stays.",
                    );
                });
                ui.checkbox(&mut it.required, "");
                // Type to search, as every doc type picker (owner 2026-11-05: long lists).
                let shown = if it.doc_type.is_empty() {
                    "—".to_string()
                } else {
                    it.doc_type.clone()
                };
                let current = doc_types.iter().position(|t| *t == it.doc_type);
                let (_, pick) = widgets::search_combo(
                    ui,
                    format!("todo-type-{id}-{i}"),
                    120.0,
                    shown,
                    || doc_types.to_vec(),
                    current,
                    Some("—"),
                );
                match pick {
                    Some(Some(n)) => it.doc_type = doc_types[n].clone(),
                    Some(None) => it.doc_type.clear(),
                    None => {}
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(i > 0, egui::Button::new("↑").small())
                        .clicked()
                    {
                        up = Some(i);
                    }
                    if ui
                        .add_enabled(i + 1 < n, egui::Button::new("↓").small())
                        .clicked()
                    {
                        up = Some(i + 1);
                    }
                    if ui
                        .add_enabled(!fixed && n > 1, egui::Button::new("Remove").small())
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
                ui.end_row();
            }
        });
    if let Some(i) = up {
        items.swap(i - 1, i);
    }
    if let Some(i) = remove {
        items.remove(i);
    }
    if ui.small_btn("+ Add item").clicked() {
        items.push(ItemIn::blank());
    }
}

fn items_ready(items: &[ItemIn]) -> bool {
    !items.is_empty() && items.iter().all(|i| !i.prompt.trim().is_empty())
}

// ======================================================================================
// A new list (or, from Admin › To-do templates, one sent to many clients)
// ======================================================================================

#[derive(Clone)]
struct ListForm {
    template_id: Option<String>,
    title: String,
    note: String,
    due: String,
    remind: u32,
    notify: bool,
    assigned: Option<String>,
    items: Vec<ItemIn>,
}

impl ListForm {
    fn blank() -> Self {
        Self {
            template_id: None,
            title: String::new(),
            note: String::new(),
            due: String::new(),
            remind: 0,
            notify: true,
            assigned: None,
            items: vec![ItemIn::blank()],
        }
    }

    fn from_template(t: &Template) -> Self {
        Self {
            template_id: Some(t.id.clone()),
            title: t.name.clone(),
            note: t.note.clone(),
            items: t
                .items
                .iter()
                .cloned()
                .map(|mut i| {
                    i.id = None;
                    i
                })
                .collect(),
            ..Self::blank()
        }
    }

    fn ready(&self) -> bool {
        !self.title.trim().is_empty() && due_ok(&self.due) && items_ready(&self.items)
    }

    /// The due day as sent: none when left empty.
    fn due_date(&self) -> Option<String> {
        Some(self.due.trim().to_string()).filter(|d| !d.is_empty())
    }

    fn to_new(&self, client_ids: Vec<String>) -> NewList {
        NewList {
            client_ids,
            title: self.title.trim().into(),
            note: self.note.trim().into(),
            due_date: self.due_date(),
            remind_every_days: self.remind,
            template_id: self.template_id.clone(),
            items: self.items.clone(),
            notify: self.notify,
            assigned_staff_id: self.assigned.clone(),
        }
    }

    /// Title, note, due day, reminders and who is told; `staff` may be empty (several clients:
    /// the portal checks each).
    fn details(&mut self, ui: &mut egui::Ui, id: &str, staff: &[StaffChoice]) {
        egui::Grid::new(format!("todo-form-{id}"))
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Title");
                ui.add(
                    egui::TextEdit::singleline(&mut self.title)
                        .desired_width(380.0)
                        .char_limit(200),
                );
                ui.end_row();
                ui.label("Note to the client");
                ui.add(
                    egui::TextEdit::singleline(&mut self.note)
                        .desired_width(380.0)
                        .char_limit(2000),
                );
                ui.end_row();
                ui.label("Due (YYYY-MM-DD)");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.due)
                            .hint_text("2027-03-15")
                            .desired_width(110.0),
                    );
                    if !due_ok(&self.due) {
                        ui.label(
                            RichText::new("Write the day as 2027-03-15").color(theme::error()),
                        );
                    }
                });
                ui.end_row();
                ui.label("Remind by itself");
                egui::ComboBox::from_id_salt(format!("todo-remind-{id}"))
                    .selected_text(days_words(self.remind))
                    .show_ui(ui, |ui| {
                        for d in REMIND_CHOICES {
                            ui.selectable_value(&mut self.remind, d, days_words(d));
                        }
                    });
                ui.end_row();
                ui.label("Assigned to");
                staff_combo(ui, &format!("todo-assign-{id}"), &mut self.assigned, staff);
                ui.end_row();
            });
    }
}

/// "No one" or a staff member (told by email when the client answers).
fn staff_combo(
    ui: &mut egui::Ui,
    id: &str,
    chosen: &mut Option<String>,
    staff: &[StaffChoice],
) -> bool {
    let shown = chosen
        .as_ref()
        .and_then(|c| staff.iter().find(|s| &s.id == c))
        .map_or_else(
            || {
                if chosen.is_some() {
                    "Someone".to_string()
                } else {
                    "No one".to_string()
                }
            },
            |s| s.name.clone(),
        );
    let before = chosen.clone();
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(chosen, None, "No one");
            for s in staff {
                ui.selectable_value(chosen, Some(s.id.clone()), &s.name);
            }
        })
        .response
        .on_hover_text(
            "Told by email when the client answers, and when every required item is answered",
        );
    *chosen != before
}

// ======================================================================================
// The window: one client's lists
// ======================================================================================

/// What a window read: the client's lists, who may be assigned, the templates, and the list
/// open in the window read again (`None`: none was open, or it is gone).
type Read = (
    Vec<TodoList>,
    Vec<StaffChoice>,
    Vec<Template>,
    Option<Box<TodoList>>,
);
/// The portal out of reach: why, and the copies in the client folder.
type Offline = (String, Option<Vec<CopyLine>>);

enum Done {
    /// The lists, staff and templates (`Err`: the portal is out of reach; the copies instead).
    Loaded(Result<Read, Offline>),
    /// A list read or changed: shown, and its summary replaced.
    List(Box<TodoList>, String),
    /// New lists made.
    Made(Vec<TodoList>),
    /// A list deleted.
    Deleted(String),
    /// PA-104: what a return said (and the file read).
    Proposal(Box<vf_extract::prior_return::Proposal>, String),
    Said(String),
    Failed(String),
}

enum Mode {
    View,
    New(ListForm),
    /// Changing the open list: its details and items.
    Edit(ListForm),
    /// PA-104: choosing last year's return to read (one of `returns`, or another file).
    FromReturn(Option<PathBuf>),
    /// PA-104: the items read from it, each ticked to keep.
    Review(Box<Review>),
}

/// What was read from a return, for the person to tick through.
struct Review {
    proposal: vf_extract::prior_return::Proposal,
    keep: Vec<bool>,
    /// The file read, as shown.
    source: String,
}

impl Review {
    /// The New list form, with the items kept.
    /// The items kept, as a new list. Each doc type is the firm's own (`doc_types`, the names
    /// the item editor offers): the proposal's seeded name in the firm's spelling, or none when
    /// the firm has no such type (renamed or archived), so Intake is never told a type it lacks.
    fn form(&self, doc_types: &[String]) -> ListForm {
        let firms = |name: &str| -> String {
            if name.is_empty() {
                return String::new();
            }
            if let Some(n) = doc_types.iter().find(|n| n.as_str() == name) {
                return n.clone();
            }
            let mut same = doc_types.iter().filter(|n| n.eq_ignore_ascii_case(name));
            match (same.next(), same.next()) {
                (Some(one), None) => one.clone(),
                _ => String::new(),
            }
        };
        let mut f = ListForm::blank();
        f.title = match self.proposal.ask_year() {
            Some(y) => format!("{y} tax documents"),
            None => "Tax documents".into(),
        };
        f.items = self
            .proposal
            .items
            .iter()
            .zip(&self.keep)
            .filter(|(_, k)| **k)
            .map(|(p, _)| ItemIn {
                id: None,
                kind: match p.kind {
                    vf_extract::prior_return::Kind::Document => todo::KIND_DOCUMENT.into(),
                    vf_extract::prior_return::Kind::Answer => todo::KIND_TEXT.into(),
                },
                prompt: p.prompt.chars().take(500).collect(),
                help: String::new(),
                required: true,
                doc_type: firms(&p.doc_type),
            })
            .collect();
        f
    }
}

/// Reads a return PDF's text (pdfium) and proposes items. Runs on a thread.
fn read_return(path: &std::path::Path) -> Result<vf_extract::prior_return::Proposal, String> {
    let dir =
        crate::ui::intake::pdfium_dir().ok_or("The PDF reader (pdfium.dll) was not found.")?;
    let pdfium = filer_pdf::pdf::pdfium(&dir)?;
    let pages = filer_pdf::pdf::page_texts(pdfium, path)?;
    Ok(vf_extract::prior_return::propose(&pages))
}

pub struct TodoWindow {
    client: ClientRef,
    folder: Option<String>,
    root: PathBuf,
    doc_types: Vec<String>,
    /// PA-104: the client's filed returns (PDF), newest first: (path, label).
    returns: Vec<(PathBuf, String)>,
    lists: Option<Vec<TodoList>>,
    /// The portal could not be reached: the copies in the client folder.
    offline: Option<Vec<CopyLine>>,
    staff: Vec<StaffChoice>,
    templates: Vec<Template>,
    /// The list opened (with its items and answers).
    open: Option<TodoList>,
    mode: Mode,
    job: Option<Receiver<Done>>,
    audit: AuditOut,
    copies: CopyOut,
    msg: Option<String>,
    err: Option<String>,
    confirm: Option<Confirm>,
    pub closed: bool,
}

impl TodoWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &egui::Context,
        s: &PortalSettings,
        root: PathBuf,
        client: ClientRef,
        folder: Option<String>,
        doc_types: Vec<String>,
        returns: Vec<(PathBuf, String)>,
        audit: AuditOut,
        copies: CopyOut,
    ) -> Self {
        let mut w = Self {
            client,
            folder,
            root,
            doc_types,
            returns,
            lists: None,
            offline: None,
            staff: Vec::new(),
            templates: Vec::new(),
            open: None,
            mode: Mode::View,
            job: None,
            audit,
            copies,
            msg: None,
            err: None,
            confirm: None,
            closed: false,
        };
        w.load(ctx, s);
        w
    }

    fn load(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        let s = s.clone();
        let client = self.client.clone();
        let root = self.root.clone();
        let folder = self.folder.clone();
        // Read again also reads the open list: its new answers show (walkthrough 2026-11-23:
        // they did not until the list was hidden and opened again).
        let open_id = self.open.as_ref().map(|l| l.id.clone());
        let copies = self.copies.clone();
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            // The client first (it must be on the portal), then its lists, its staff and the
            // templates together: they do not depend on each other.
            let read =
                p.put_client(&client.id, &client.code, &client.name, None)
                    .and_then(|()| {
                        std::thread::scope(|sc| {
                            let staff = sc.spawn(|| p.todo_staff(&client.id));
                            let templates = sc.spawn(|| p.todo_templates());
                            let open = open_id.as_deref().map(|id| {
                                let p = &p;
                                sc.spawn(move || p.todo_list(id))
                            });
                            let lists = p.todo_lists(&client.id);
                            let open = match open.map(joined) {
                                Some(Ok(l)) => {
                                    if let Some(c) =
                                        copy_of(&root, folder.as_deref(), &l, &p.actor_name())
                                    {
                                        let _ = copies.0.send(c);
                                    }
                                    Some(Box::new(l))
                                }
                                // Deleted elsewhere since: shown no more.
                                Some(Err(vf_portal::PortalError::Http { status: 404, .. }))
                                | None => None,
                                Some(Err(e)) => return Err(e),
                            };
                            Ok((lists?, joined(staff)?, joined(templates)?, open))
                        })
                    });
            Done::Loaded(read.map_err(|e| {
                // Only when the portal cannot be reached: a refusal (to-do lists off, a bad
                // key, an older portal) is said as it is (QA 2026-11-23).
                let copies = matches!(e, vf_portal::PortalError::Network(_)).then(|| {
                    folder
                        .as_deref()
                        .map(|f| todo::read_copies(&root, f))
                        .unwrap_or_default()
                });
                (e.message(), copies)
            }))
        }));
    }

    /// Runs a call that answers one list: it is copied to the client folder and shown.
    fn run_list(
        &mut self,
        ctx: &egui::Context,
        s: &PortalSettings,
        entry: Option<AuditEntry>,
        msg: String,
        call: impl FnOnce(&vf_portal::Portal) -> vf_portal::api::Result<TodoList> + Send + 'static,
    ) {
        let root = self.root.clone();
        let folder = self.folder.clone();
        let copies = self.copies.clone();
        self.job(
            ctx,
            s,
            entry,
            move |p| {
                let l = call(p)?;
                if let Some(c) = copy_of(&root, folder.as_deref(), &l, &p.actor_name()) {
                    let _ = copies.0.send(c);
                }
                Ok(l)
            },
            move |l| Done::List(Box::new(l), msg),
        );
    }

    /// Runs one portal call on a thread: on success its audit row is sent and `ok` says what
    /// the window shows; a failure shows the portal's message.
    fn job<T: 'static>(
        &mut self,
        ctx: &egui::Context,
        s: &PortalSettings,
        entry: Option<AuditEntry>,
        call: impl FnOnce(&vf_portal::Portal) -> vf_portal::api::Result<T> + Send + 'static,
        ok: impl FnOnce(T) -> Done + Send + 'static,
    ) {
        let s = s.clone();
        let audit = self.audit.clone();
        self.err = None;
        self.msg = None;
        self.job = Some(spawn(ctx, move || match call(&core::client(&s)) {
            Ok(v) => {
                if let Some(e) = entry {
                    audit.send(e);
                }
                ok(v)
            }
            Err(e) => Done::Failed(e.message()),
        }));
    }

    /// The open list's title (for audit rows).
    fn open_title(&self) -> String {
        self.open
            .as_ref()
            .map(|l| l.title.clone())
            .unwrap_or_default()
    }

    fn entry(&self, action: &str, list: &str, title: &str, more: serde_json::Value) -> AuditEntry {
        let mut detail = serde_json::json!({
            "client_id": self.client.id,
            "client": self.client.name,
            "list_id": list,
            "title": title,
        });
        if let (Some(d), Some(m)) = (detail.as_object_mut(), more.as_object()) {
            for (k, v) in m {
                d.insert(k.clone(), v.clone());
            }
        }
        AuditEntry::new(action, None, detail)
    }

    fn take_done(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.job = None;
        // A pending Cancel/Delete question belongs to the list it was asked for: whatever the
        // window shows next, it is not carried over (QA 2026-11-23).
        self.confirm = None;
        match done {
            Done::Loaded(Ok((lists, staff, templates, open))) => {
                self.open = open.map(|l| *l);
                self.lists = Some(lists);
                self.staff = staff;
                self.templates = templates;
                self.offline = None;
            }
            Done::Loaded(Err((e, copies))) => {
                self.err = Some(e);
                self.offline = copies;
            }
            Done::List(l, msg) => {
                let l = *l;
                if let Some(ls) = &mut self.lists {
                    // The list of lists holds summaries (no items).
                    let summary = TodoList {
                        items: Vec::new(),
                        ..l.clone()
                    };
                    match ls.iter_mut().find(|x| x.id == l.id) {
                        Some(x) => *x = summary,
                        None => ls.insert(0, summary),
                    }
                }
                if matches!(self.mode, Mode::Edit(_)) {
                    self.mode = Mode::View;
                }
                self.open = Some(l);
                if !msg.is_empty() {
                    self.msg = Some(msg);
                }
            }
            Done::Made(made) => {
                let n = made.len();
                if let Some(ls) = &mut self.lists {
                    for l in made.iter().filter(|l| l.client_id == self.client.id) {
                        ls.insert(
                            0,
                            TodoList {
                                items: Vec::new(),
                                ..l.clone()
                            },
                        );
                    }
                }
                self.open = made.into_iter().find(|l| l.client_id == self.client.id);
                self.mode = Mode::View;
                self.msg = Some(if n == 1 {
                    "The list was sent.".into()
                } else {
                    format!("Sent to {n} clients.")
                });
            }
            Done::Deleted(id) => {
                if let Some(ls) = &mut self.lists {
                    ls.retain(|l| l.id != id);
                }
                if self.open.as_ref().is_some_and(|l| l.id == id) {
                    self.open = None;
                }
                self.msg = Some("The list was deleted.".into());
            }
            // Only while the window still waits for it.
            Done::Proposal(..) if !matches!(self.mode, Mode::FromReturn(_)) => {}
            Done::Proposal(p, source) => {
                if p.looks_empty() {
                    self.err = Some(format!(
                        "{source} does not read as an UltraTax return (no wage, 1099, K-1 or statement pages were found; a scanned return has no text to read). Choose another file."
                    ));
                } else {
                    let keep = vec![true; p.items.len()];
                    self.mode = Mode::Review(Box::new(Review {
                        proposal: *p,
                        keep,
                        source,
                    }));
                }
            }
            Done::Said(m) => self.msg = Some(m),
            Done::Failed(e) => self.err = Some(e),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, s: &PortalSettings) {
        self.take_done();
        let busy = self.job.is_some();
        let mut open_win = true;
        let mut act: Option<Act> = None;
        egui::Window::new(format!("To-do lists — {}", self.client.name))
            .id(egui::Id::unique("portal-todo"))
            .open(&mut open_win)
            .collapsible(false)
            .default_width(780.0)
            .default_height(760.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| act = self.body(ui, busy));
            });
        if !open_win {
            self.closed = true;
        }
        if let Some(a) = act {
            self.apply(ctx, s, a);
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<Act> {
        let mut act = None;
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        done_line(ui, self.msg.as_deref());
        if let Some(e) = &self.err {
            ui.label(RichText::new(e).color(theme::error()));
        }
        if let Some(copies) = &self.offline {
            ui.label(widgets::caption(
                "The portal cannot be reached: the copies kept in the client's folder are shown, as last read.",
            ));
            if copies.is_empty() {
                ui.label("No copy of a list is kept in this client's folder.");
            }
            for c in copies {
                ui.separator();
                ui.label(
                    RichText::new(format!("{} — {}", c.list.title, c.list.status_words())).strong(),
                );
                ui.label(widgets::caption(format!(
                    "As read {} by {}",
                    widgets::local_time(&c.read_at),
                    c.read_by
                )));
                list_answers(ui, &c.list);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Try the portal again"))
                .clicked()
            {
                act = Some(Act::Reload);
            }
            return act;
        }
        let Some(lists) = &self.lists else {
            if !busy {
                ui.label(widgets::caption("Reading the portal…"));
            }
            return None;
        };
        // ---- New list
        match &mut self.mode {
            Mode::New(f) => {
                ui.label(RichText::new("New list").strong());
                f.details(ui, "new", &self.staff);
                ui.add_space(4.0);
                items_editor(ui, "new", &mut f.items, &HashSet::new(), &self.doc_types);
                ui.checkbox(
                    &mut f.notify,
                    "Tell the client's people now (email or text, as each chose)",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy && f.ready(), widgets::primary_button("Send the list"))
                        .clicked()
                    {
                        act = Some(Act::Create(f.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        act = Some(Act::Cancel);
                    }
                });
                ui.separator();
            }
            Mode::FromReturn(chosen) => {
                ui.label(RichText::new("New list from last year's return").strong());
                ui.label(widgets::caption(
                    "Vibe Files reads the return's supporting reports and statements (wages, 1099-R, \
                     dividends, brokers, K-1s, 1098s, estimated payments) and proposes an item for \
                     each. Nothing leaves this PC until you send the list.",
                ));
                if self.returns.is_empty() {
                    ui.label("No filed return was found for this client (doc type \"Return copy — federal\", or a PDF named for a 1040 return).");
                }
                for (path, label) in &self.returns {
                    ui.radio_value(chosen, Some(path.clone()), label);
                }
                if let Some(p) = chosen
                    .as_ref()
                    .filter(|p| !self.returns.iter().any(|(r, _)| r == *p))
                {
                    ui.label(format!("Chosen: {}", p.display()));
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Choose another PDF…"))
                        .clicked()
                    {
                        let mut d = rfd::FileDialog::new()
                            .set_title("Last year's return (PDF)")
                            .add_filter("PDF", &["pdf"]);
                        if let Some(f) = &self.folder {
                            d = d.set_directory(self.root.join(f));
                        }
                        if let Some(p) = d.pick_file() {
                            *chosen = Some(p);
                        }
                    }
                    if ui
                        .add_enabled(
                            !busy && chosen.is_some(),
                            widgets::primary_button("Read it"),
                        )
                        .clicked()
                        && let Some(p) = chosen.clone()
                    {
                        act = Some(Act::ReadReturn(p));
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                        act = Some(Act::Cancel);
                    }
                });
                ui.separator();
            }
            Mode::Review(r) => {
                ui.label(RichText::new("Items read from last year's return").strong());
                ui.label(widgets::caption(format!(
                    "From {}{}. Untick what you don't need; you can change any wording next.",
                    r.source,
                    r.proposal
                        .year
                        .map(|y| format!(" ({y} return)"))
                        .unwrap_or_default()
                )));
                let mut group = None;
                for (i, p) in r.proposal.items.iter().enumerate() {
                    if group != Some(p.group) {
                        group = Some(p.group);
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(p.group.words().to_uppercase())
                                .small()
                                .color(theme::ink_muted()),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut r.keep[i], &p.prompt);
                        let what = match p.kind {
                            vf_extract::prior_return::Kind::Document if p.doc_type.is_empty() => {
                                "Documents".to_string()
                            }
                            vf_extract::prior_return::Kind::Document => {
                                format!("Documents · {}", p.doc_type)
                            }
                            vf_extract::prior_return::Kind::Answer => "Answer".to_string(),
                        };
                        ui.label(widgets::caption(what))
                            .on_hover_text(format!("Found in: {}", p.source));
                    });
                }
                let n = r.keep.iter().filter(|k| **k).count();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy && n > 0,
                            widgets::primary_button(format!("Use these {n} items")),
                        )
                        .clicked()
                    {
                        act = Some(Act::UseProposal(r.form(&self.doc_types)));
                    }
                    if ui.button("Cancel").clicked() {
                        act = Some(Act::Cancel);
                    }
                });
                ui.separator();
            }
            Mode::View | Mode::Edit(_) => {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("+ New list"))
                        .clicked()
                    {
                        act = Some(Act::NewBlank);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("From last year's return…"))
                        .on_hover_text("Read the UltraTax return filed for this client and propose an item for each W-2, 1099, K-1 and more")
                        .clicked()
                    {
                        act = Some(Act::FromReturn);
                    }
                    if !self.templates.is_empty() && !busy {
                        egui::ComboBox::from_id_salt("todo-from-template")
                            .selected_text("From a template…")
                            .show_ui(ui, |ui| {
                                for t in &self.templates {
                                    if ui.selectable_label(false, &t.name).clicked() {
                                        act = Some(Act::NewFrom(t.clone()));
                                    }
                                }
                            });
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Read again"))
                        .clicked()
                    {
                        act = Some(Act::Reload);
                    }
                });
            }
        }
        // ---- The lists
        if lists.is_empty() {
            ui.label("No list has been sent to this client yet.");
        }
        let open_id = self.open.as_ref().map(|l| l.id.clone());
        egui::Grid::new("todo-lists")
            .num_columns(5)
            .striped(true)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for l in lists {
                    ui.label(RichText::new(&l.title).strong());
                    ui.label(format!("{} of {}", l.done, l.total));
                    ui.label(
                        l.due_date
                            .clone()
                            .map_or_else(|| "no due day".into(), |d| format!("due {d}")),
                    );
                    ui.label(l.status_words());
                    let shown = open_id.as_deref() == Some(l.id.as_str());
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(if shown { "Hide" } else { "Open" }).small(),
                        )
                        .clicked()
                    {
                        act = Some(if shown {
                            Act::Hide
                        } else {
                            Act::Open(l.id.clone())
                        });
                    }
                    ui.end_row();
                }
            });
        // ---- The list opened
        if let Some(l) = &self.open {
            ui.separator();
            ui.label(RichText::new(&l.title).strong().size(15.0));
            ui.label(widgets::caption(format!(
                "{} · {} of {} done · sent {} by {} · due {} · reminders: {}",
                l.status_words(),
                l.done,
                l.total,
                widgets::local_time(&l.created_at),
                l.created_by,
                l.due_date.as_deref().unwrap_or("—"),
                days_words(l.remind_every_days).to_lowercase(),
            )));
            if !l.note.is_empty() {
                ui.label(format!("Note: {}", l.note));
            }
            if let Mode::Edit(f) = &mut self.mode {
                let locked: HashSet<String> = l
                    .items
                    .iter()
                    .filter(|i| !i.responses.is_empty())
                    .map(|i| i.id.clone())
                    .collect();
                f.details(ui, "edit", &self.staff);
                items_editor(ui, "edit", &mut f.items, &locked, &self.doc_types);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy && f.ready(), widgets::primary_button("Save"))
                        .clicked()
                    {
                        act = Some(Act::Save(l.id.clone(), f.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        act = Some(Act::Cancel);
                    }
                });
                return act;
            }
            ui.horizontal(|ui| {
                ui.label("Assigned to");
                let mut who = l.assigned_staff_id.clone();
                ui.add_enabled_ui(!busy && l.status == "open", |ui| {
                    if staff_combo(ui, "todo-open-assign", &mut who, &self.staff) {
                        act = Some(Act::Assign(l.id.clone(), who.clone()));
                    }
                });
            });
            list_answers(ui, l);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Download report…")).clicked() {
                    act = Some(Act::Report(l.id.clone(), l.title.clone(), false));
                }
                if ui
                    .add_enabled(!busy && self.folder.is_some(), egui::Button::new("File report in the client folder"))
                    .on_hover_text("Puts the report in the Portal inbox, named for this client: Intake files it")
                    .clicked()
                {
                    act = Some(Act::Report(l.id.clone(), l.title.clone(), true));
                }
                if l.status == "open" {
                    if ui.add_enabled(!busy, egui::Button::new("Remind now")).clicked() {
                        act = Some(Act::Remind(l.id.clone(), l.title.clone()));
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Edit")).clicked() {
                        act = Some(Act::Edit);
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Close")).clicked() {
                        act = Some(Act::Status(l.id.clone(), ListStatus::Closed));
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Cancel list")).clicked() {
                        self.confirm = Some(Confirm::Cancel(l.id.clone()));
                    }
                } else if ui.add_enabled(!busy, egui::Button::new("Open again")).clicked() {
                    act = Some(Act::Status(l.id.clone(), ListStatus::Open));
                }
                if l.unanswered()
                    && ui
                        .add_enabled(!busy, egui::Button::new("Delete"))
                        .on_hover_text("Only while nobody has answered; a list with answers is cancelled instead")
                        .clicked()
                {
                    self.confirm = Some(Confirm::Delete(l.id.clone()));
                }
            });
            if let Some(c) = self.confirm.clone().filter(|c| c.list() == l.id) {
                let (question, button) = match c {
                    Confirm::Delete(_) => ("Delete this list? Nobody has answered it.", "Delete"),
                    Confirm::Cancel(_) => (
                        "Cancel this list? The client no longer sees it; its answers are kept.",
                        "Cancel list",
                    ),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(question).color(theme::error()));
                    if ui.button(button).clicked() {
                        act = Some(match c {
                            Confirm::Delete(id) => Act::Delete(id),
                            Confirm::Cancel(id) => Act::Status(id, ListStatus::Cancelled),
                        });
                        self.confirm = None;
                    }
                    if ui.button("Keep it").clicked() {
                        self.confirm = None;
                    }
                });
            }
        }
        act
    }

    fn apply(&mut self, ctx: &egui::Context, s: &PortalSettings, a: Act) {
        match a {
            Act::Reload => {
                self.confirm = None;
                self.offline = None;
                self.err = None;
                self.msg = None;
                self.load(ctx, s);
            }
            Act::NewBlank => self.mode = Mode::New(ListForm::blank()),
            Act::FromReturn => {
                self.err = None;
                self.mode = Mode::FromReturn(self.returns.first().map(|(p, _)| p.clone()));
            }
            Act::ReadReturn(path) => {
                self.err = None;
                self.msg = None;
                let shown = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.job = Some(spawn(ctx, move || match read_return(&path) {
                    Ok(p) => Done::Proposal(Box::new(p), shown),
                    Err(e) => Done::Failed(format!("{shown} could not be read: {e}")),
                }));
            }
            Act::UseProposal(f) => self.mode = Mode::New(f),
            Act::NewFrom(t) => self.mode = Mode::New(ListForm::from_template(&t)),
            Act::Cancel => self.mode = Mode::View,
            Act::Hide => {
                self.confirm = None;
                self.open = None;
            }
            Act::Open(id) => {
                self.confirm = None;
                self.run_list(ctx, s, None, String::new(), move |p| p.todo_list(&id));
            }
            Act::Edit => {
                if let Some(l) = &self.open {
                    self.mode = Mode::Edit(ListForm {
                        template_id: l.template_id.clone(),
                        title: l.title.clone(),
                        note: l.note.clone(),
                        due: l.due_date.clone().unwrap_or_default(),
                        remind: l.remind_every_days,
                        notify: false,
                        assigned: l.assigned_staff_id.clone(),
                        items: l.items.iter().map(ItemIn::from_item).collect(),
                    });
                }
            }
            Act::Save(id, f) => {
                let change = ListChange {
                    title: Some(f.title.trim().into()),
                    note: Some(f.note.trim().into()),
                    due_date: Some(f.due_date()),
                    remind_every_days: Some(f.remind),
                    assigned_staff_id: Some(f.assigned.clone()),
                    items: Some(f.items.clone()),
                    status: None,
                };
                let e = self.entry(
                    "portal_todo_change",
                    &id,
                    &f.title,
                    serde_json::json!({ "items": f.items.len() }),
                );
                self.run_list(ctx, s, Some(e), "Saved.".into(), move |p| {
                    p.change_list(&id, &change)
                });
            }
            Act::Assign(id, who) => {
                let name = who
                    .as_ref()
                    .and_then(|w| self.staff.iter().find(|x| &x.id == w))
                    .map(|x| x.name.clone());
                let e = self.entry(
                    "portal_todo_change",
                    &id,
                    &self.open_title(),
                    serde_json::json!({ "assigned_to": name }),
                );
                let msg = match &name {
                    Some(n) => format!("{n} is told when the client answers."),
                    None => "No one is assigned now.".into(),
                };
                let change = ListChange {
                    assigned_staff_id: Some(who),
                    ..Default::default()
                };
                self.run_list(ctx, s, Some(e), msg, move |p| p.change_list(&id, &change));
            }
            Act::Status(id, status) => {
                let (code, action, msg) = status.words();
                let e = self.entry(action, &id, &self.open_title(), serde_json::json!({}));
                let change = ListChange {
                    status: Some(code.into()),
                    ..Default::default()
                };
                self.run_list(ctx, s, Some(e), msg.into(), move |p| {
                    p.change_list(&id, &change)
                });
            }
            Act::Create(f) => {
                let new = f.to_new(vec![self.client.id.clone()]);
                let e = self.entry(
                    "portal_todo_create",
                    "",
                    &f.title,
                    serde_json::json!({ "items": f.items.len() }),
                );
                let s = s.clone();
                let client = self.client.clone();
                let root = self.root.clone();
                let folder = self.folder.clone();
                let audit = self.audit.clone();
                let copies = self.copies.clone();
                self.err = None;
                self.msg = None;
                self.job = Some(spawn(ctx, move || {
                    let p = core::client(&s);
                    match p
                        .put_client(&client.id, &client.code, &client.name, None)
                        .and_then(|()| p.create_lists(&new))
                    {
                        Ok(made) => {
                            for l in &made {
                                let mut row = e.clone();
                                row.detail["list_id"] = serde_json::json!(l.id);
                                audit.send(row);
                                if let Some(c) =
                                    copy_of(&root, folder.as_deref(), l, &p.actor_name())
                                {
                                    let _ = copies.0.send(c);
                                }
                            }
                            Done::Made(made)
                        }
                        Err(e) => Done::Failed(e.message()),
                    }
                }));
            }
            Act::Delete(id) => {
                let e = self.entry(
                    "portal_todo_delete",
                    &id,
                    &self.open_title(),
                    serde_json::json!({}),
                );
                let gone = id.clone();
                self.job(
                    ctx,
                    s,
                    Some(e),
                    move |p| p.delete_list(&id),
                    move |()| Done::Deleted(gone),
                );
            }
            Act::Remind(id, title) => {
                let e = self.entry("portal_todo_remind", &id, &title, serde_json::json!({}));
                self.job(
                    ctx,
                    s,
                    Some(e),
                    move |p| p.remind(&id),
                    |n| {
                        Done::Said(match n {
                            0 => "Nobody could be told (no open items, or nobody with access)."
                                .into(),
                            1 => "1 person was reminded.".into(),
                            n => format!("{n} people were reminded."),
                        })
                    },
                );
            }
            Act::Report(id, title, to_inbox) => self.report(ctx, s, id, title, to_inbox),
        }
    }

    fn report(
        &mut self,
        ctx: &egui::Context,
        s: &PortalSettings,
        id: String,
        title: String,
        to_inbox: bool,
    ) {
        let file = format!("To-do report - {}.pdf", file_part(&title));
        // Only a whole report is placed: into the Portal inbox by vf-portal, or over the file
        // chosen by an atomic write (QA 2026-11-23: a failed download had removed the file).
        let dest: Option<PathBuf> = if to_inbox {
            None
        } else {
            let Some(p) = rfd::FileDialog::new()
                .set_title("Save the to-do report")
                .set_file_name(file_part(&format!("{} - {file}", self.client.name)))
                .add_filter("PDF", &["pdf"])
                .save_file()
            else {
                return;
            };
            if inside(&self.root, &p) {
                self.err = Some(
                    "Save the report outside the cabinet, or use File report in the client folder (Intake files it)."
                        .into(),
                );
                return;
            }
            Some(p)
        };
        let s = s.clone();
        let audit = self.audit.clone();
        let mut e = self.entry(
            "portal_todo_report",
            &id,
            &title,
            serde_json::json!({ "to_inbox": to_inbox }),
        );
        let client = self.client.clone();
        let root = self.root.clone();
        self.err = None;
        self.msg = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let placed = match &dest {
                None => p
                    .file_report(&root, &client.name, &client.code, &file, &id)
                    .map_err(|err| err.message()),
                Some(dest) => p
                    .report_pdf(&id)
                    .map_err(|err| err.message())
                    .and_then(|bytes| {
                        filer_core::datafile::write_atomic(dest, &bytes)
                            .map(|()| dest.clone())
                            .map_err(|err| format!("The report could not be saved: {err}"))
                    }),
            };
            match placed {
                Ok(at) => {
                    e.detail["saved_as"] = at.display().to_string().into();
                    audit.send(e);
                    Done::Said(if dest.is_none() {
                        "The report is in the Portal inbox: Intake files it into the client's folder."
                            .into()
                    } else {
                        format!("Saved {}.", at.display())
                    })
                }
                Err(err) => Done::Failed(err),
            }
        }));
    }
}

/// Text fit for a Windows file name: characters it cannot have become spaces.
fn file_part(s: &str) -> String {
    // Filer's file-name cleaning (the name always goes on with " - …" or ".pdf" after it, so
    // trailing dots and reserved names do not arise).
    let t = filer_core::template::sanitize_value(s);
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(120).collect();
    if t.is_empty() { "list".into() } else { t }
}

/// `path` is inside the cabinet (where only Transactions write documents).
fn inside(root: &std::path::Path, path: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = path.parent().map(canon).unwrap_or_default();
    let root = canon(root);
    dir.starts_with(&root)
}

/// Each item, and every answer with when, who and from where.
fn list_answers(ui: &mut egui::Ui, l: &TodoList) {
    for (n, it) in l.items.iter().enumerate() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{}. {}", n + 1, it.prompt)).strong());
            ui.label(widgets::caption(format!(
                "{}{}{} — {}",
                kind_words(&it.kind).to_lowercase(),
                if it.required { ", required" } else { "" },
                if it.doc_type.is_empty() {
                    String::new()
                } else {
                    format!(", {}", it.doc_type)
                },
                state_words(&it.state)
            )));
        });
        for a in &it.responses {
            ui.indent(format!("todo-a-{}", a.id), |ui| {
                ui.label(a.words());
                let contact = [a.person.email.as_deref(), a.person.phone.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(widgets::caption(format!(
                    "{} · {}{} · IP {}",
                    widgets::local_time(&a.at),
                    if a.person.name.is_empty() {
                        "?"
                    } else {
                        &a.person.name
                    },
                    if contact.is_empty() {
                        String::new()
                    } else {
                        format!(" ({contact})")
                    },
                    a.ip
                )))
                .on_hover_text(format!("Browser: {}", a.user_agent));
                if let Some(h) = &a.file_sha256 {
                    ui.label(widgets::caption(format!("SHA-256 {h}")));
                }
            });
        }
    }
}

enum Act {
    Reload,
    NewBlank,
    NewFrom(Template),
    Cancel,
    Hide,
    Open(String),
    Edit,
    Save(String, ListForm),
    Assign(String, Option<String>),
    Status(String, ListStatus),
    Create(ListForm),
    Delete(String),
    Remind(String, String),
    Report(String, String, bool),
    /// PA-104.
    FromReturn,
    ReadReturn(PathBuf),
    UseProposal(ListForm),
}

/// What a list is set to by Close, Cancel and Open again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListStatus {
    Open,
    Closed,
    Cancelled,
}

impl ListStatus {
    /// (the portal's word, the audit action, what the window says).
    fn words(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Open => ("open", "portal_todo_reopen", "Opened again."),
            Self::Closed => (
                "closed",
                "portal_todo_close",
                "Closed: the client can no longer answer.",
            ),
            Self::Cancelled => ("cancelled", "portal_todo_cancel", "Cancelled."),
        }
    }
}

/// A change asked to be confirmed first, for a list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Confirm {
    Delete(String),
    Cancel(String),
}

impl Confirm {
    fn list(&self) -> &str {
        match self {
            Self::Delete(id) | Self::Cancel(id) => id,
        }
    }
}

// ======================================================================================
// Admin › To-do templates
// ======================================================================================

enum TDone {
    Loaded(Result<Vec<Template>, String>),
    Saved(Template),
    Deleted(String),
    Sent(usize),
    Failed(String),
}

pub struct TemplatesPage {
    loaded: bool,
    templates: Vec<Template>,
    /// Clients on the cabinet for "Send to clients…", each with its lowercased "name code"
    /// (made once, not every frame).
    clients: Vec<(ClientRef, String)>,
    editing: Option<Template>,
    sending: Option<(Template, ListForm, HashSet<String>, String)>,
    job: Option<Receiver<TDone>>,
    msg: Option<String>,
    err: Option<String>,
    confirm_delete: Option<String>,
    /// Audit rows when the app's channel is not given (tests): recorded by the page.
    audit: AuditOut,
    audit_rx: Receiver<AuditEntry>,
    /// The app's portal channel (recorded every frame, whatever page is shown: QA 2026-11-23).
    pub app_audit: Option<AuditOut>,
    /// The cabinet's portal settings, read at most every 30 s (often on a share).
    settings: Option<PortalSettings>,
    read_at: Option<std::time::Instant>,
    /// Doc type names for the pickers (not archived), read with the templates; shared so the
    /// page borrows them each frame without a copy.
    doc_types: std::rc::Rc<[String]>,
}

impl Default for TemplatesPage {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            loaded: false,
            templates: Vec::new(),
            clients: Vec::new(),
            editing: None,
            sending: None,
            job: None,
            msg: None,
            err: None,
            confirm_delete: None,
            audit: AuditOut::new(tx),
            audit_rx: rx,
            app_audit: None,
            settings: None,
            read_at: None,
            doc_types: std::rc::Rc::from([]),
        }
    }
}

/// A PDF named for a filed 1040 return: the file's own name (not a folder's, a client code's),
/// and not an estimate voucher, payment voucher, amended return or organizer.
fn return_name(rel: &str) -> bool {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel).to_lowercase();
    let squeezed: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    (name.contains("1040") || name.contains("tax return"))
        && !["1040es", "1040x", "voucher", "organizer", "estimate"]
            .iter()
            .any(|w| squeezed.contains(w))
}

#[cfg(test)]
mod return_name_tests {
    #[test]
    fn only_a_return_by_its_own_name() {
        assert!(super::return_name(
            "Sample (10401)/Income Tax/2025/2025 1040 Return.pdf"
        ));
        assert!(!super::return_name(
            "Sample (10401)/Income Tax/2025/W-2.pdf"
        ));
        assert!(!super::return_name(
            "Sample (1042)/Income Tax/2026/2026 1040-ES Vouchers.pdf"
        ));
        assert!(!super::return_name(
            "Sample (1042)/Income Tax/2025/2025 1040 Organizer.pdf"
        ));
        assert!(!super::return_name(
            "Sample (1042)/Income Tax/2024/2024 1040-X.pdf"
        ));
    }
}

/// PA-104: the client's filed returns (PDFs of doc type "Return copy — federal", or named for a
/// 1040 / tax return), newest tax year first: (path, label).
pub fn filed_returns(
    conn: &vf_core::db::Connection,
    root: &std::path::Path,
    client_id: &str,
) -> Vec<(PathBuf, String)> {
    conn.prepare(
        "SELECT d.rel_path, d.tax_year, t.name = 'Return copy — federal' FROM document d LEFT JOIN doc_type t ON t.id = d.doc_type_id \
         WHERE d.client_id = ?1 AND d.deleted_at IS NULL AND lower(d.rel_path) LIKE '%.pdf' \
         AND (t.name = 'Return copy — federal' OR lower(d.rel_path) LIKE '%1040%' \
              OR lower(d.rel_path) LIKE '%tax return%') \
         ORDER BY (t.name = 'Return copy — federal') DESC, d.tax_year DESC, d.updated_at DESC LIMIT 40",
    )
    .and_then(|mut st| {
        st.query_map([client_id], |r| {
            let rel: String = r.get(0)?;
            let year: Option<i64> = r.get(1)?;
            let typed: bool = r.get::<_, Option<bool>>(2)?.unwrap_or(false);
            Ok((rel, year, typed))
        })?
        .collect::<Result<Vec<_>, _>>()
    })
    .unwrap_or_default()
    .into_iter()
    .filter(|(rel, _, typed)| *typed || return_name(rel))
    .take(12)
    .map(|(rel, year, _)| {
        let label = match year {
            Some(y) => format!("{y}: {rel}"),
            None => rel.clone(),
        };
        (root.join(rel), label)
    })
    .collect()
}

/// Doc type names offered in the item editor: not deleted, not archived (migration 011).
pub fn doc_type_names(conn: &vf_core::db::Connection) -> Vec<String> {
    conn.prepare(
        "SELECT name FROM doc_type WHERE deleted_at IS NULL AND archived = 0 ORDER BY sort, name",
    )
    .and_then(|mut st| {
        st.query_map([], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()
    })
    .unwrap_or_default()
}

/// The cabinet's portal settings (`None` until the firm sets a portal).
pub fn cabinet_settings(root: &std::path::Path) -> Option<PortalSettings> {
    let id = vf_fs::cabinet::read(root).ok()?.cabinet_id;
    vf_portal::settings::load(root, &id)
}

impl TemplatesPage {
    /// Read the templates, clients and settings again (the page was entered, or Read again).
    pub fn reread(&mut self) {
        self.loaded = false;
        self.read_at = None;
    }

    fn take(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(d) = rx.try_recv() else { return };
        self.job = None;
        match d {
            TDone::Loaded(Ok(t)) => self.templates = t,
            TDone::Loaded(Err(e)) | TDone::Failed(e) => self.err = Some(e),
            TDone::Saved(t) => {
                match self.templates.iter_mut().find(|x| x.id == t.id) {
                    Some(x) => *x = t,
                    None => self.templates.push(t),
                }
                self.templates.sort_by_key(|t| t.name.to_lowercase());
                self.editing = None;
                self.msg = Some("Template saved.".into());
            }
            TDone::Deleted(id) => {
                self.templates.retain(|t| t.id != id);
                self.msg = Some("Template deleted; lists already sent are kept.".into());
            }
            TDone::Sent(n) => {
                self.sending = None;
                self.msg = Some(format!(
                    "Sent to {n} client{}.",
                    if n == 1 { "" } else { "s" }
                ));
            }
        }
    }
}

/// Admin › To-do templates: saved lists, and sending one to many clients at once.
pub fn templates_page(
    ui: &mut egui::Ui,
    page: &mut TemplatesPage,
    env: &mut crate::ui::browse::Env,
) {
    page.take();
    if page
        .read_at
        .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(30))
    {
        page.read_at = Some(std::time::Instant::now());
        page.settings = cabinet_settings(env.root);
    }
    let settings = page.settings.clone();
    let rows: Vec<AuditEntry> = page.audit_rx.try_iter().collect();
    if let Some(e) = crate::ui::portal::record(env, "portal_todo_templates", rows) {
        page.err = Some(e.trim().to_string());
    }
    let audit = page.app_audit.clone().unwrap_or_else(|| page.audit.clone());
    let ctx = ui.ctx().clone();
    ui.heading("To-do templates");
    let Some(s) = settings.filter(|s| s.todo_lists) else {
        ui.label("To-do lists are off for this cabinet. Turn them on in Admin › Portal.");
        return;
    };
    ui.label(widgets::caption(
        "A template is a list you send often (the year-end documents for a 1040, a loan package). \
         Send one to a client from the client's To-do lists…, or to many clients here. Each client \
         gets a list of their own.",
    ));
    if !page.loaded {
        page.loaded = true;
        let s2 = s.clone();
        page.job = Some(spawn(&ctx, move || {
            TDone::Loaded(core::client(&s2).todo_templates().map_err(|e| e.message()))
        }));
        page.clients = crate::ui::portal::load_clients(env)
            .into_iter()
            .map(|c| {
                let key = format!("{} {}", c.name, c.code).to_lowercase();
                (c, key)
            })
            .collect();
        page.doc_types = doc_type_names(env.conn).into();
    }
    let doc_types = std::rc::Rc::clone(&page.doc_types);
    let busy = page.job.is_some();
    if ui
        .add_enabled(!busy, egui::Button::new("Read again").small())
        .clicked()
    {
        page.err = None;
        page.reread();
    }
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Working…");
        });
    }
    done_line(ui, page.msg.as_deref());
    if let Some(e) = &page.err {
        ui.label(RichText::new(e).color(theme::error()));
    }
    // ---- The templates
    let mut edit: Option<Template> = None;
    let mut send: Option<Template> = None;
    let mut delete: Option<String> = None;
    if page.templates.is_empty() && !busy {
        ui.label("No templates yet.");
    }
    egui::Grid::new("todo-templates")
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for t in &page.templates {
                ui.label(RichText::new(&t.name).strong());
                ui.label(format!(
                    "{} item{}",
                    t.items.len(),
                    if t.items.len() == 1 { "" } else { "s" }
                ));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Edit").small())
                        .clicked()
                    {
                        edit = Some(t.clone());
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Send to clients…").small())
                        .clicked()
                    {
                        send = Some(t.clone());
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Delete").small())
                        .clicked()
                    {
                        page.confirm_delete = Some(t.id.clone());
                    }
                });
                if page.confirm_delete.as_deref() == Some(t.id.as_str()) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Delete it?").color(theme::error()));
                        if ui.small_btn("Delete").clicked() {
                            delete = Some(t.id.clone());
                        }
                        if ui.small_btn("Keep").clicked() {
                            page.confirm_delete = None;
                        }
                    });
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
    if page.editing.is_none()
        && page.sending.is_none()
        && ui
            .add_enabled(!busy, egui::Button::new("+ New template"))
            .clicked()
    {
        page.editing = Some(Template {
            id: String::new(),
            name: String::new(),
            note: String::new(),
            items: vec![ItemIn::blank()],
        });
    }
    if let Some(t) = edit {
        page.sending = None;
        page.editing = Some(t);
    }
    if let Some(t) = send {
        page.editing = None;
        let form = ListForm::from_template(&t);
        page.sending = Some((t, form, HashSet::new(), String::new()));
    }
    if let Some(id) = delete {
        page.confirm_delete = None;
        let s2 = s.clone();
        let name = page
            .templates
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let a = audit.clone();
        page.job = Some(spawn(&ctx, move || {
            match core::client(&s2).delete_template(&id) {
                Ok(()) => {
                    a.send(AuditEntry::new(
                        "portal_todo_template_delete",
                        None,
                        serde_json::json!({ "template_id": id, "name": name }),
                    ));
                    TDone::Deleted(id)
                }
                Err(e) => TDone::Failed(e.message()),
            }
        }));
    }
    // ---- Editing one
    let mut save: Option<Template> = None;
    let mut stop = false;
    if let Some(t) = &mut page.editing {
        ui.separator();
        ui.label(
            RichText::new(if t.id.is_empty() {
                "New template"
            } else {
                "Edit template"
            })
            .strong(),
        );
        egui::Grid::new("todo-template-form")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut t.name)
                        .desired_width(380.0)
                        .char_limit(200),
                );
                ui.end_row();
                ui.label("Note to the client");
                ui.add(
                    egui::TextEdit::singleline(&mut t.note)
                        .desired_width(380.0)
                        .char_limit(2000),
                );
                ui.end_row();
            });
        items_editor(ui, "template", &mut t.items, &HashSet::new(), &doc_types);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy && !t.name.trim().is_empty() && items_ready(&t.items),
                    widgets::primary_button("Save template"),
                )
                .clicked()
            {
                save = Some(t.clone());
            }
            if ui.button("Cancel").clicked() {
                stop = true;
            }
        });
    }
    if stop {
        page.editing = None;
    }
    if let Some(t) = save {
        let s2 = s.clone();
        let a = audit.clone();
        page.job = Some(spawn(&ctx, move || {
            match core::client(&s2).save_template(&t) {
                Ok(saved) => {
                    a.send(AuditEntry::new(
                    "portal_todo_template_save",
                    None,
                    serde_json::json!({ "template_id": saved.id, "name": saved.name, "items": saved.items.len() }),
                ));
                    TDone::Saved(saved)
                }
                Err(e) => TDone::Failed(e.message()),
            }
        }));
    }
    // ---- Sending one to many clients
    let mut go: Option<NewList> = None;
    let mut stop = false;
    if let Some((t, form, chosen, find)) = &mut page.sending {
        ui.separator();
        ui.label(RichText::new(format!("Send “{}” to clients", t.name)).strong());
        ui.horizontal(|ui| {
            ui.label("Find a client");
            ui.add(egui::TextEdit::singleline(find).desired_width(240.0));
        });
        let q = find.trim().to_lowercase();
        let shown: Vec<&ClientRef> = page
            .clients
            .iter()
            .filter(|(_, key)| q.is_empty() || key.contains(&q))
            .map(|(c, _)| c)
            .collect();
        // Only the rows in view are laid out (hundreds of clients).
        let row = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical()
            .id_salt("todo-send-clients")
            .max_height(220.0)
            .show_rows(ui, row, shown.len(), |ui, range| {
                for c in &shown[range] {
                    let mut on = chosen.contains(&c.id);
                    if ui
                        .checkbox(&mut on, format!("{}  {}", c.name, c.code))
                        .changed()
                    {
                        if on {
                            chosen.insert(c.id.clone());
                        } else {
                            chosen.remove(&c.id);
                        }
                    }
                }
            });
        ui.label(format!("{} chosen", chosen.len()));
        form.details(ui, "send", &[]);
        ui.label(widgets::caption(
            "Assign someone from a client's own To-do lists… (staff differ by client).",
        ));
        ui.checkbox(
            &mut form.notify,
            "Tell each client's people now (email or text, as each chose)",
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy && !chosen.is_empty() && form.ready(),
                    widgets::primary_button(format!(
                        "Send to {} client{}",
                        chosen.len(),
                        if chosen.len() == 1 { "" } else { "s" }
                    )),
                )
                .clicked()
            {
                go = Some(form.to_new(chosen.iter().cloned().collect()));
            }
            if ui.button("Cancel").clicked() {
                stop = true;
            }
        });
    }
    if stop {
        page.sending = None;
    }
    if let Some(new) = go {
        let s2 = s.clone();
        let a = audit.clone();
        let clients: Vec<ClientRef> = page
            .clients
            .iter()
            .filter(|(c, _)| new.client_ids.contains(&c.id))
            .map(|(c, _)| c.clone())
            .collect();
        page.job = Some(spawn(&ctx, move || {
            let p = core::client(&s2);
            // Each client must be on the portal before a list can be sent to it: a few at a
            // time (hundreds of clients one by one took minutes).
            let failed = std::thread::scope(|sc| {
                let per = clients.len().div_ceil(6).max(1);
                let handles: Vec<_> = clients
                    .chunks(per)
                    .map(|part| {
                        let p = &p;
                        sc.spawn(move || {
                            part.iter().find_map(|c| {
                                p.put_client(&c.id, &c.code, &c.name, None)
                                    .err()
                                    .map(|e| format!("{}: {}", c.name, e.message()))
                            })
                        })
                    })
                    .collect();
                handles.into_iter().find_map(|h| h.join().ok().flatten())
            });
            if let Some(e) = failed {
                return TDone::Failed(e);
            }
            match p.create_lists(&new) {
                Ok(made) => {
                    for l in &made {
                        a.send(AuditEntry::new(
                            "portal_todo_create",
                            None,
                            serde_json::json!({ "client_id": l.client_id, "list_id": l.id, "title": l.title, "items": l.total }),
                        ));
                    }
                    TDone::Sent(made.len())
                }
                Err(e) => TDone::Failed(e.message()),
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PA-104: an invented return PDF read through pdfium proposes its items, and the ones kept
    /// become the New list form.
    #[test]
    fn last_years_return_becomes_a_new_list() {
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let pdf = t.path().join("2025 1040 Tax Return.pdf");
        crate::ui::test_util::write_lines_pdf(&pdf, crate::ui::test_util::SAMPLE_RETURN);
        let p = read_return(&pdf).unwrap();
        assert_eq!(p.year, Some(2025));
        let prompts: Vec<&str> = p.items.iter().map(|i| i.prompt.as_str()).collect();
        for want in [
            "W-2 from Sample Accounting, P.C. (Avery)",
            "W-2 from Example R-I School District (Morgan)",
            "1099-R (pension or annuity) from State Teachers Retirement Sys (Avery)",
            "1099-DIV from Sample Funds, account ending 4321",
            "Form 1098 (mortgage interest) from Sample Mortgage Loan",
            "K-1 from Example Holdings, LLC (partnership)",
        ] {
            assert!(prompts.contains(&want), "missing {want:?} in {prompts:#?}");
        }
        let mut keep = vec![true; p.items.len()];
        keep[0] = false;
        let r = Review {
            proposal: p,
            keep,
            source: "x".into(),
        };
        let f = r.form(&["W-2".to_string(), "Organizer".to_string()]);
        assert_eq!(f.title, "2026 tax documents");
        assert_eq!(f.items.len(), r.proposal.items.len() - 1);
        assert!(
            f.items
                .iter()
                .any(|i| i.doc_type == "W-2" && i.kind == todo::KIND_DOCUMENT)
        );
        assert!(f.items.iter().any(|i| i.kind == todo::KIND_TEXT));
        // A type the firm does not have is left off (Intake would drop it).
        assert!(!f.items.iter().any(|i| i.doc_type == "1099-R"));
        assert!(f.ready());
    }

    #[test]
    fn a_list_read_is_copied_into_the_client_folder_with_an_audit_row() {
        const C1: &str = "018f0000-0000-7000-8000-000000000001";
        const C2: &str = "018f0000-0000-7000-8000-000000000002";
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let bind = |folder: &str, client: &str| {
            let v = root.join(folder).join("_Vibe");
            std::fs::create_dir_all(&v).unwrap();
            std::fs::write(
                v.join("client.json"),
                format!(
                    r#"{{"version":1,"client_id":"{client}","firm_id":"{C2}","tax_software_id":"1","display_name_at_creation":"x","created_at":"2026-11-01T00:00:00Z","created_by":null}}"#
                ),
            )
            .unwrap();
        };
        bind("Smith, John (1001)", C1);
        bind("Smith Farms LLC (1002)", C2);
        let mut conn =
            vf_core::db::open(&t.path().join("catalog.db"), vf_core::db::Storage::Local).unwrap();
        let list: TodoList = serde_json::from_str(
            r#"{"id":"l-1","clientId":"018f0000-0000-7000-8000-000000000001","title":"2026 tax documents","dueDate":null,
                "status":"open","complete":false,"done":0,"total":1,
                "createdAt":"2026-11-09T15:00:00Z","items":[]}"#,
        )
        .unwrap();
        let c = copy_of(root, Some("Smith, John (1001)"), &list, "PC1-kurt").unwrap();
        write_copies(&mut conn, root, "dev-1", vec![c]).unwrap();
        let path = root.join("Smith, John (1001)/_Vibe/todo/l-1.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        // Read again unchanged: no new line.
        assert!(copy_of(root, Some("Smith, John (1001)"), &list, "PC2-ann").is_none());
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'portal_todo_copy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // A client with no folder has no copy; nor does a folder renamed away (gone) or one bound
        // to another client: no folder is made again unbound.
        assert!(copy_of(root, None, &list, "x").is_none());
        assert!(copy_of(root, Some("Smith, John (old)"), &list, "x").is_none());
        assert!(!root.join("Smith, John (old)").exists());
        assert!(copy_of(root, Some("Smith Farms LLC (1002)"), &list, "x").is_none());
        // A record only: not on the Ctrl+Z stack.
        assert!(
            vf_fs::fileops::undo_stack(&conn, "dev-1")
                .unwrap()
                .is_empty()
        );
    }
}
