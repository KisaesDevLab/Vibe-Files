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
use crate::ui::portal::{AuditOut, ClientRef};
use crate::ui::widgets::UiExt as _;
use crate::ui::{theme, widgets};

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
    let mut tx = vf_core::tx::Transaction::new("portal_todo_copy");
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

/// Runs `f` on a thread; the window repaints when it is done.
fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    f: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(f());
        ctx.request_repaint();
    });
    rx
}

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
    let sentinel = root
        .join(folder)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join("client.json");
    let bound = std::fs::read(&sentinel)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| {
            v.get("client_id")
                .and_then(|c| c.as_str())
                .map(str::to_lowercase)
        });
    if bound.as_deref() != Some(l.client_id.to_lowercase().as_str()) {
        return None;
    }
    let path = todo::copy_path(root, folder, &l.id);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let line = todo::copy_line(&existing, l, &vf_core::now_rfc3339(), by)?;
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
                egui::ComboBox::from_id_salt(format!("todo-type-{id}-{i}"))
                    .selected_text(if it.doc_type.is_empty() {
                        "—"
                    } else {
                        it.doc_type.as_str()
                    })
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut it.doc_type, String::new(), "—");
                        for t in doc_types {
                            ui.selectable_value(&mut it.doc_type, t.clone(), t);
                        }
                    });
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

    fn to_new(&self, client_ids: Vec<String>) -> NewList {
        NewList {
            client_ids,
            title: self.title.trim().into(),
            note: self.note.trim().into(),
            due_date: Some(self.due.trim().to_string()).filter(|d| !d.is_empty()),
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

/// What a window read: the client's lists, who may be assigned, the templates.
type Read = (Vec<TodoList>, Vec<StaffChoice>, Vec<Template>);
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
    Said(String),
    Failed(String),
}

enum Mode {
    View,
    New(ListForm),
    /// Changing the open list: its details and items.
    Edit(ListForm),
}

pub struct TodoWindow {
    client: ClientRef,
    folder: Option<String>,
    root: PathBuf,
    doc_types: Vec<String>,
    lists: Option<Vec<TodoList>>,
    /// The portal could not be reached: the copies in the client folder.
    offline: Option<(String, Vec<CopyLine>)>,
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
    confirm: Option<(String, &'static str)>,
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
        audit: AuditOut,
        copies: CopyOut,
    ) -> Self {
        let mut w = Self {
            client,
            folder,
            root,
            doc_types,
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
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            let read = p
                .put_client(&client.id, &client.code, &client.name, None)
                .and_then(|()| {
                    Ok((
                        p.todo_lists(&client.id)?,
                        p.todo_staff(&client.id)?,
                        p.todo_templates()?,
                    ))
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
        let s = s.clone();
        let root = self.root.clone();
        let folder = self.folder.clone();
        let audit = self.audit.clone();
        let copies = self.copies.clone();
        self.err = None;
        self.msg = None;
        self.job = Some(spawn(ctx, move || {
            let p = core::client(&s);
            match call(&p) {
                Ok(l) => {
                    if let Some(e) = entry {
                        audit.send(e);
                    }
                    if let Some(c) = copy_of(&root, folder.as_deref(), &l, &p.actor_name()) {
                        let _ = copies.0.send(c);
                    }
                    Done::List(Box::new(l), msg)
                }
                Err(e) => Done::Failed(e.message()),
            }
        }));
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
            Done::Loaded(Ok((lists, staff, templates))) => {
                self.lists = Some(lists);
                self.staff = staff;
                self.templates = templates;
                self.offline = None;
            }
            Done::Loaded(Err((e, copies))) => {
                self.err = Some(e);
                if let Some(copies) = copies {
                    self.offline = Some((
                        "The portal cannot be reached: the copies kept in the client's folder are shown, as last read."
                            .into(),
                        copies,
                    ));
                }
            }
            Done::List(l, msg) => {
                let l = *l;
                if let Some(ls) = &mut self.lists {
                    match ls.iter_mut().find(|x| x.id == l.id) {
                        Some(x) => {
                            *x = TodoList {
                                items: Vec::new(),
                                ..l.clone()
                            };
                        }
                        None => ls.insert(
                            0,
                            TodoList {
                                items: Vec::new(),
                                ..l.clone()
                            },
                        ),
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
        widgets::message_line(ui, self.msg.as_deref());
        if let Some(e) = &self.err {
            ui.label(RichText::new(e).color(theme::error()));
        }
        if let Some((why, copies)) = &self.offline {
            ui.label(widgets::caption(why));
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
            Mode::View | Mode::Edit(_) => {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("+ New list"))
                        .clicked()
                    {
                        act = Some(Act::NewBlank);
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
                        act = Some(Act::Status(l.id.clone(), "closed"));
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Cancel list")).clicked() {
                        self.confirm = Some((l.id.clone(), "cancel"));
                    }
                } else if ui.add_enabled(!busy, egui::Button::new("Open again")).clicked() {
                    act = Some(Act::Status(l.id.clone(), "open"));
                }
                if l.unanswered()
                    && ui
                        .add_enabled(!busy, egui::Button::new("Delete"))
                        .on_hover_text("Only while nobody has answered; a list with answers is cancelled instead")
                        .clicked()
                {
                    self.confirm = Some((l.id.clone(), "delete"));
                }
            });
            if let Some((id, what)) = self.confirm.clone().filter(|(id, _)| *id == l.id) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if what == "delete" {
                            "Delete this list? Nobody has answered it."
                        } else {
                            "Cancel this list? The client no longer sees it; its answers are kept."
                        })
                        .color(theme::error()),
                    );
                    if ui
                        .button(if what == "delete" {
                            "Delete"
                        } else {
                            "Cancel list"
                        })
                        .clicked()
                    {
                        act = Some(if what == "delete" {
                            Act::Delete(id.clone())
                        } else {
                            Act::Status(id.clone(), "cancelled")
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
                self.load(ctx, s);
            }
            Act::NewBlank => self.mode = Mode::New(ListForm::blank()),
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
                    due_date: Some(Some(f.due.trim().to_string()).filter(|d| !d.is_empty())),
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
                let title = self
                    .open
                    .as_ref()
                    .map(|l| l.title.clone())
                    .unwrap_or_default();
                let e = self.entry(
                    "portal_todo_change",
                    &id,
                    &title,
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
                let title = self
                    .open
                    .as_ref()
                    .map(|l| l.title.clone())
                    .unwrap_or_default();
                let action = match status {
                    "open" => "portal_todo_reopen",
                    "closed" => "portal_todo_close",
                    _ => "portal_todo_cancel",
                };
                let e = self.entry(action, &id, &title, serde_json::json!({}));
                let msg = match status {
                    "open" => "Opened again.",
                    "closed" => "Closed: the client can no longer answer.",
                    _ => "Cancelled.",
                };
                let change = ListChange {
                    status: Some(status.into()),
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
                let title = self
                    .open
                    .as_ref()
                    .map(|l| l.title.clone())
                    .unwrap_or_default();
                let e = self.entry("portal_todo_delete", &id, &title, serde_json::json!({}));
                let s = s.clone();
                let audit = self.audit.clone();
                self.job = Some(spawn(ctx, move || {
                    match core::client(&s).delete_list(&id) {
                        Ok(()) => {
                            audit.send(e);
                            Done::Deleted(id)
                        }
                        Err(e) => Done::Failed(e.message()),
                    }
                }));
            }
            Act::Remind(id, title) => {
                let e = self.entry("portal_todo_remind", &id, &title, serde_json::json!({}));
                let s = s.clone();
                let audit = self.audit.clone();
                self.job = Some(spawn(ctx, move || match core::client(&s).remind(&id) {
                    Ok(n) => {
                        audit.send(e);
                        Done::Said(match n {
                            0 => "Nobody could be told (no open items, or nobody with access)."
                                .into(),
                            1 => "1 person was reminded.".into(),
                            n => format!("{n} people were reminded."),
                        })
                    }
                    Err(e) => Done::Failed(e.message()),
                }));
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
        // Downloaded first to a `.part` file; only a whole report replaces anything (QA
        // 2026-11-23: a failed download had removed the file being saved over).
        let (part, finish): (PathBuf, Finish) = if to_inbox {
            let name = vf_portal::inbox::inbox_name(&self.client.name, &self.client.code, &file);
            let tmp = vf_portal::inbox::tmp_dir(&self.root);
            if let Err(e) = std::fs::create_dir_all(&tmp) {
                self.err = Some(format!("The Portal inbox cannot be written: {e}"));
                return;
            }
            let stem: String = id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            (
                tmp.join(format!("todo-report-{stem}-{}.part", vf_core::new_id())),
                Finish::Inbox(vf_portal::inbox::inbox(&self.root), name),
            )
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
            let mut part = p.clone().into_os_string();
            part.push(".part");
            (PathBuf::from(part), Finish::Replace(p))
        };
        let s = s.clone();
        let audit = self.audit.clone();
        let saved = match &finish {
            Finish::Replace(p) => p.display().to_string(),
            Finish::Inbox(dir, name) => dir.join(name).display().to_string(),
        };
        let e = self.entry(
            "portal_todo_report",
            &id,
            &title,
            serde_json::json!({ "saved_as": saved, "to_inbox": to_inbox }),
        );
        self.err = None;
        self.msg = None;
        self.job = Some(spawn(ctx, move || {
            let _ = std::fs::remove_file(&part);
            if let Err(err) = core::client(&s).download_report(&id, &part) {
                return Done::Failed(err.message());
            }
            let placed = match &finish {
                // Never over a file there: a free name, as every Portal inbox arrival.
                Finish::Inbox(dir, name) => vf_portal::inbox::move_into(&part, dir, name),
                Finish::Replace(dest) => std::fs::rename(&part, dest).map(|()| dest.clone()),
            };
            match placed {
                Ok(at) => {
                    audit.send(e);
                    Done::Said(if to_inbox {
                        "The report is in the Portal inbox: Intake files it into the client's folder."
                            .into()
                    } else {
                        format!("Saved {}.", at.display())
                    })
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&part);
                    Done::Failed(format!("The report could not be saved: {err}"))
                }
            }
        }));
    }
}

/// Text fit for a Windows file name: characters it cannot have become spaces.
fn file_part(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(120).collect();
    if t.is_empty() { "list".into() } else { t }
}

/// Where a downloaded report goes once whole.
enum Finish {
    /// Into the Portal inbox (folder, client-named file name), under a free name.
    Inbox(PathBuf, String),
    /// Over the file the person chose (they agreed to replace it in the dialog).
    Replace(PathBuf),
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
    Status(String, &'static str),
    Create(ListForm),
    Delete(String),
    Remind(String, String),
    Report(String, String, bool),
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
    /// Clients on the cabinet: (id, code, name), for "Send to clients…".
    clients: Vec<(String, String, String)>,
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
    /// Doc type names for the pickers (not archived), read with the templates.
    doc_types: Vec<String>,
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
            doc_types: Vec::new(),
        }
    }
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
    if !rows.is_empty() {
        let mut tx = vf_core::tx::Transaction::new("portal_todo_templates");
        for r in rows {
            tx = tx.audit(r);
        }
        if let Err(e) = tx.commit(
            env.conn,
            &mut vf_core::tx::NoFileOps,
            &vf_core::tx::Actor::device(env.device_id),
        ) {
            page.err = Some(format!("This was NOT recorded in the audit trail: {e}"));
        }
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
        page.clients = env
            .conn
            .prepare("SELECT id, ext_id, name FROM client WHERE deleted_at IS NULL ORDER BY name")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();
        page.doc_types = doc_type_names(env.conn);
    }
    let doc_types = page.doc_types.clone();
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
    widgets::message_line(ui, page.msg.as_deref());
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
        egui::ScrollArea::vertical()
            .id_salt("todo-send-clients")
            .max_height(220.0)
            .show(ui, |ui| {
                for (id, code, name) in page.clients.iter().filter(|(_, c, n)| {
                    q.is_empty() || n.to_lowercase().contains(&q) || c.to_lowercase().contains(&q)
                }) {
                    let mut on = chosen.contains(id);
                    if ui.checkbox(&mut on, format!("{name}  {code}")).changed() {
                        if on {
                            chosen.insert(id.clone());
                        } else {
                            chosen.remove(id);
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
        let clients: Vec<(String, String, String)> = page
            .clients
            .iter()
            .filter(|(id, _, _)| new.client_ids.contains(id))
            .cloned()
            .collect();
        page.job = Some(spawn(&ctx, move || {
            let p = core::client(&s2);
            // Each client must be on the portal before a list can be sent to it.
            for (id, code, name) in &clients {
                if let Err(e) = p.put_client(id, code, name, None) {
                    return TDone::Failed(format!("{name}: {}", e.message()));
                }
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

    #[test]
    fn a_list_read_is_copied_into_the_client_folder_with_an_audit_row() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let bind = |folder: &str, client: &str| {
            let v = root.join(folder).join("_Vibe");
            std::fs::create_dir_all(&v).unwrap();
            std::fs::write(
                v.join("client.json"),
                format!(r#"{{"client_id":"{client}"}}"#),
            )
            .unwrap();
        };
        bind("Smith, John (1001)", "c-1");
        bind("Smith Farms LLC (1002)", "c-2");
        let mut conn =
            vf_core::db::open(&t.path().join("catalog.db"), vf_core::db::Storage::Local).unwrap();
        let list: TodoList = serde_json::from_str(
            r#"{"id":"l-1","clientId":"c-1","title":"2026 tax documents","dueDate":null,
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
        assert!(vf_fs::fileops::NOT_ON_UNDO_STACK.contains(&"portal_todo_copy"));
    }
}
