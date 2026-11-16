//! Finding documents from Browse (owner 2026-11-15, Q-113: the Search tab is gone). One box
//! for words in the file name or its folders, the details filters, and "Save search…". What is
//! found shows in Browse's own list (`Scope::Found`), like a saved search; Browse runs the
//! query on a thread.

use egui::RichText;
use vf_core::browse::Snapshot;
use vf_index::search::{DocType, Query, Scope, Year};

use crate::ui::widgets::UiExt as _;
use crate::ui::{theme, widgets};

/// The most documents a find lists. More than that is not a list anyone reads: the line above
/// the list says to add a word or a filter.
pub const LIMIT: usize = 10_000;

/// What "This client" and "This folder" mean: the node selected in Browse's tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    /// (client id, label).
    pub client: Option<(String, String)>,
    /// (cabinet-relative folder, its year, label).
    pub folder: Option<(String, Option<i32>, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Where {
    #[default]
    Cabinet,
    Client,
    Folder,
}

/// The Find documents panel of Browse: what is typed and chosen in it.
#[derive(Default)]
pub struct Find {
    pub text: String,
    pub scope: Where,
    /// Filters (the query's metadata fields; text and scope are filled in when it runs).
    pub filters: Query,
    year_text: String,
    /// The "Save search" name being typed.
    save_name: Option<String>,
    pub notice: Option<String>,
}

/// What the panel asks Browse to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Find the documents of this query.
    Run(Query),
    /// Keep this query under this name, for the whole firm.
    Save(String, Query),
    /// Close the panel.
    Close,
}

impl Find {
    /// The query as the box, scope and filters say.
    pub fn query(&self, ctx: &Context) -> Query {
        let mut q = self.filters.clone();
        q.text = self.text.trim().to_string();
        q.scope = Scope::Cabinet;
        match self.scope {
            Where::Cabinet => {}
            Where::Client => {
                if let Some((id, _)) = &ctx.client {
                    q.scope = Scope::Client(id.clone());
                }
            }
            Where::Folder => {
                if let Some((f, year, _)) = &ctx.folder {
                    q.scope = Scope::Folder(f.clone());
                    if q.year.is_none() {
                        q.year = Some(year.map_or(Year::Permanent, Year::Year));
                    }
                }
            }
        }
        q
    }

    /// A search was saved under `name`: said in the panel, and the name box closes.
    pub fn saved(&mut self, name: &str) {
        self.notice = Some(format!(
            "Saved \"{}\". It is under Saved searches on the left.",
            name.trim()
        ));
        self.save_name = None;
    }
}

/// The panel, above Browse's list. `listing`: the cabinet is still being listed behind the
/// window (PA-60), so what is found may be incomplete (owner 2026-11-14, Q-106). `busy`: a find
/// is running.
pub fn panel(
    ui: &mut egui::Ui,
    s: &mut Find,
    snap: &Snapshot,
    ctx: &Context,
    listing: Option<&str>,
    busy: bool,
) -> Option<Ask> {
    let mut ask = None;
    egui::Frame::new()
        .fill(theme::pane())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            if let Some(line) = listing {
                ui.label(RichText::new(line).color(theme::ink_secondary()));
            }
            ui.horizontal_wrapped(|ui| {
                let label = ui.label("Words");
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut s.text)
                            .hint_text("in the file name or its folders…")
                            .desired_width(300.0),
                    )
                    .labelled_by(label.id);
                crate::ui::widgets::clear_x(ui, &r, &mut s.text);
                let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.separator();
                ui.radio_value(&mut s.scope, Where::Cabinet, "Cabinet");
                let client = ctx.client.as_ref().map(|c| c.1.as_str());
                ui.add_enabled_ui(client.is_some(), |ui| {
                    ui.radio_value(&mut s.scope, Where::Client, "This client")
                        .on_hover_text(client.unwrap_or("Select a client in the tree"));
                });
                let folder = ctx.folder.as_ref().map(|f| f.2.as_str());
                ui.add_enabled_ui(folder.is_some(), |ui| {
                    ui.radio_value(&mut s.scope, Where::Folder, "This folder")
                        .on_hover_text(folder.unwrap_or("Select a folder in the tree"));
                });
                let q = s.query(ctx);
                let can = !q.is_empty() && !busy;
                let find = ui
                    .add_enabled(can, widgets::primary_button("Find"))
                    .on_disabled_hover_text("Type a word or choose a filter first.");
                if can && (find.clicked() || entered) {
                    s.notice = None;
                    ask = Some(Ask::Run(q));
                }
                if ui
                    .button("Close")
                    .on_hover_text("Close Find documents")
                    .clicked()
                {
                    ask = Some(Ask::Close);
                }
            });
            filters(ui, s, snap);
            if let Some(a) = save_row(ui, s, ctx) {
                ask = Some(a);
            }
        });
    ask
}

/// `Save search…`: names the current query; Browse lists it under Saved searches for the
/// whole firm.
fn save_row(ui: &mut egui::Ui, s: &mut Find, ctx: &Context) -> Option<Ask> {
    let q = s.query(ctx);
    let mut ask = None;
    ui.horizontal(|ui| {
        match &mut s.save_name {
            None => {
                if ui
                    .add_enabled(!q.is_empty(), egui::Button::new("Save search…"))
                    .on_hover_text(
                        "Saved searches are listed on the left for everyone in the firm.",
                    )
                    .clicked()
                {
                    s.save_name = Some(String::new());
                    s.notice = None;
                }
            }
            Some(name) => {
                let l = ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("e.g. 1099s received 2025")
                        .desired_width(220.0),
                )
                .labelled_by(l.id);
                if ui.add(widgets::primary_button("Save")).clicked() {
                    ask = Some(Ask::Save(name.clone(), q.clone()));
                } else if ui.button("Cancel").clicked() {
                    s.save_name = None;
                }
            }
        }
        if let Some(n) = &s.notice {
            ui.label(widgets::caption(n));
        }
    });
    ask
}

fn filters(ui: &mut egui::Ui, s: &mut Find, snap: &Snapshot) {
    let f = &mut s.filters;
    ui.horizontal_wrapped(|ui| {
        let client_name = f
            .client
            .as_ref()
            .and_then(|id| snap.clients.iter().find(|c| &c.id == id))
            .map_or("Any client".to_string(), |c| {
                format!("{} ({})", c.name, c.ext_id)
            });
        ui.horizontal(|ui| {
            let l = ui.label("Client");
            let current = f
                .client
                .as_ref()
                .and_then(|id| snap.clients.iter().position(|c| &c.id == id));
            let (r, pick) = widgets::search_combo(
                ui,
                "search-client",
                180.0,
                client_name,
                || {
                    snap.clients
                        .iter()
                        .map(|c| format!("{} ({})", c.name, c.ext_id))
                        .collect()
                },
                current,
                Some("Any client"),
            );
            r.labelled_by(l.id);
            if let Some(k) = pick {
                f.client = k.map(|k| snap.clients[k].id.clone());
            }
        });
        ui.horizontal(|ui| {
            let yl = ui.label("Year");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut s.year_text)
                        .hint_text("any")
                        .desired_width(70.0),
                )
                .labelled_by(yl.id)
                .on_hover_text("2025, Permanent, or empty for any year")
                .changed()
            {
                let t = s.year_text.trim();
                f.year = if t.eq_ignore_ascii_case("permanent") || t.eq_ignore_ascii_case("p") {
                    Some(Year::Permanent)
                } else {
                    t.parse().ok().map(Year::Year)
                };
            }
        });
        let dt_name = match &f.doc_type {
            None => "Any doc type".to_string(),
            Some(DocType::Needed) => "Needs doc type".to_string(),
            Some(DocType::Is(id)) => snap
                .doc_types
                .iter()
                .find(|(i, _)| i == id)
                .map_or("?".into(), |(_, n)| n.clone()),
        };
        ui.horizontal(|ui| {
            let l = ui.label("Doc type");
            // "Needs doc type", then the doc types (archived ones stay searchable: older
            // documents have them; marked).
            let current = match &f.doc_type {
                None => None,
                Some(DocType::Needed) => Some(0),
                Some(DocType::Is(id)) => snap
                    .doc_types
                    .iter()
                    .position(|(i, _)| i == id)
                    .map(|k| k + 1),
            };
            let (r, pick) = widgets::search_combo(
                ui,
                "search-doctype",
                160.0,
                dt_name,
                || {
                    std::iter::once("Needs doc type".to_string())
                        .chain(snap.doc_types.iter().map(|(id, name)| {
                            if snap.archived_types.contains(id) {
                                format!("{name} (archived)")
                            } else {
                                name.clone()
                            }
                        }))
                        .collect()
                },
                current,
                Some("Any doc type"),
            );
            r.labelled_by(l.id);
            if let Some(k) = pick {
                f.doc_type = match k {
                    None => None,
                    Some(0) => Some(DocType::Needed),
                    Some(k) => Some(DocType::Is(snap.doc_types[k - 1].0.clone())),
                };
            }
        });
        combo(
            ui,
            "search-status",
            "Status",
            "Any status",
            &mut f.status,
            &vf_fs::docmeta::STATUSES,
        );
        combo(
            ui,
            "search-source",
            "Source",
            "Any source",
            &mut f.source,
            &vf_fs::docmeta::SOURCES,
        );
        text_filter(ui, "Filed from", &mut f.filed_from, "YYYY-MM-DD");
        text_filter(ui, "to", &mut f.filed_to, "YYYY-MM-DD");
        text_filter(ui, "Filed by", &mut f.filed_by, "anyone");
        text_filter(ui, "Tag", &mut f.tag, "any");
        if *f != Query::default() && ui.small_btn("Clear filters").clicked() {
            *f = Query::default();
            s.year_text.clear();
        }
    });
}

fn combo(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    any: &'static str,
    v: &mut Option<String>,
    opts: &[(&str, &'static str)],
) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(widgets::label_of(opts, v.as_deref(), any))
            .show_ui(ui, |ui| {
                ui.selectable_value(v, None, any);
                for (key, name) in opts {
                    ui.selectable_value(v, Some((*key).to_string()), *name);
                }
            })
            .response
            .labelled_by(l.id);
    });
}

fn text_filter(ui: &mut egui::Ui, label: &str, v: &mut Option<String>, hint: &str) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        let mut t = v.clone().unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut t)
                    .hint_text(hint)
                    .desired_width(100.0),
            )
            .labelled_by(l.id)
            .changed()
        {
            *v = (!t.trim().is_empty()).then(|| t.trim().to_string());
        }
    });
}
