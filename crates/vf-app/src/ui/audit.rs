//! Admin › Audit trail (PHASES P05 step 2): filter by user, client, document, action and
//! date; export CSV or PDF; verify the hash chain now. [`ChainCheck`] also verifies the chain
//! when the cabinet opens and again every week it stays open; a broken chain shows a banner on
//! every tab.

use std::time::{Duration, Instant};

use egui::{RichText, TextEdit};
use vf_core::audit::{self, Filter, Listed, VerifyReport};
use vf_core::db::Connection;

use crate::ui::browse::Env;
use crate::ui::{theme, widgets, wizard::ERROR};

/// Rows shown on the page (exports take every matching row).
const SHOWN: usize = 500;
const WEEK: Duration = Duration::from_secs(7 * 24 * 3600);

/// The last chain verification and when it ran.
#[derive(Debug, Clone)]
pub struct ChainCheck {
    pub at: Instant,
    /// Local time it ran, for the page.
    pub when: String,
    pub result: Result<VerifyReport, String>,
}

impl ChainCheck {
    pub fn run(conn: &Connection) -> Self {
        let result = audit::verify(conn).map_err(|e| e.to_string());
        match &result {
            Ok(r) => match &r.broken {
                None => tracing::info!(rows = r.rows, "audit chain verified"),
                Some((seq, why)) => tracing::error!(seq, why, "audit chain broken"),
            },
            Err(e) => tracing::warn!(kind = crate::logging::kind(e), "audit chain not checked"),
        }
        ChainCheck {
            at: Instant::now(),
            when: widgets::local_time(&vf_core::now_rfc3339()),
            result,
        }
    }

    /// Runs again when a week has passed.
    pub fn tick(&mut self, conn: &Connection) {
        if self.at.elapsed() >= WEEK {
            *self = Self::run(conn);
        }
    }

    /// The banner text when the chain is broken or could not be read.
    pub fn problem(&self) -> Option<String> {
        match &self.result {
            Ok(r) => r.broken.as_ref().map(|(seq, why)| {
                format!(
                    "The audit trail does not verify at row {seq}: {why}. Someone may have edited the catalog outside Vibe Files. See Admin › Audit trail."
                )
            }),
            Err(e) => Some(format!("The audit trail could not be checked: {e}")),
        }
    }

    pub fn summary(&self) -> String {
        match &self.result {
            Ok(r) if r.ok() => format!("Chain verified {}: {} rows, intact.", self.when, r.rows),
            _ => self.problem().unwrap_or_default(),
        }
    }
}

/// The page's state.
#[derive(Default)]
pub struct AuditPage {
    pub filter: Filter,
    /// What the filters matched (newest [`SHOWN`]); `None`: query again.
    rows: Option<Vec<Listed>>,
    actions: Vec<String>,
    users: Vec<String>,
    /// Live clients: (uuid, "Name (ID)").
    clients: Vec<(String, String)>,
    pub notice: Option<String>,
    pub error: Option<String>,
    /// "Verify now" pressed: the app runs the check and stores it in its [`ChainCheck`].
    pub verify_requested: bool,
}

impl AuditPage {
    fn load(&mut self, conn: &Connection) {
        match audit::query(conn, &self.filter, Some(SHOWN)) {
            Ok(r) => self.rows = Some(r),
            Err(e) => {
                self.error = Some(e.to_string());
                self.rows = Some(vec![]);
            }
        }
        if let Ok((a, u)) = audit::choices(conn) {
            (self.actions, self.users) = (a, u);
        }
        self.clients = conn
            .prepare(
                "SELECT id, name || ' (' || ext_id || ')' FROM client WHERE deleted_at IS NULL
                 ORDER BY name",
            )
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default();
    }

    /// Re-query on the next frame (the catalog changed).
    pub fn refresh(&mut self) {
        self.rows = None;
    }

    /// Every matching row, oldest first.
    fn all(&self, conn: &Connection) -> Result<Vec<Listed>, String> {
        audit::query(conn, &self.filter, None).map_err(|e| e.to_string())
    }

    /// What the filters say, for the PDF's subtitle and file names.
    fn describe(&self) -> String {
        let f = &self.filter;
        let mut parts = Vec::new();
        if let Some(c) = &f.client_id
            && let Some((_, name)) = self.clients.iter().find(|(id, _)| id == c)
        {
            parts.push(format!("client {name}"));
        }
        if !f.user.trim().is_empty() {
            parts.push(format!("user {}", f.user.trim()));
        }
        if !f.doc.trim().is_empty() {
            parts.push(format!("document \"{}\"", f.doc.trim()));
        }
        if let Some(a) = &f.action {
            parts.push(format!("action {a}"));
        }
        match (f.from.trim(), f.to.trim()) {
            ("", "") => {}
            (a, "") => parts.push(format!("from {a}")),
            ("", b) => parts.push(format!("to {b}")),
            (a, b) => parts.push(format!("{a} to {b}")),
        }
        if parts.is_empty() {
            "all rows".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// The audit export as PDF bytes.
pub fn pdf(rows: &[Listed], what: &str, chain: Option<&ChainCheck>) -> Vec<u8> {
    use vf_pdf::report::Column;
    let cols = [
        Column {
            heading: "seq",
            width: 6,
        },
        Column {
            heading: "time (UTC)",
            width: 20,
        },
        Column {
            heading: "user",
            width: 18,
        },
        Column {
            heading: "action",
            width: 16,
        },
        Column {
            heading: "client",
            width: 22,
        },
        Column {
            heading: "document",
            width: 40,
        },
        Column {
            heading: "detail",
            width: 42,
        },
    ];
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|l| {
            let c = audit::cells(l);
            vec![
                c[0].clone(),
                c[1].clone(),
                if c[2].is_empty() {
                    c[3].clone()
                } else {
                    c[2].clone()
                },
                c[4].clone(),
                c[5].clone(),
                c[6].clone(),
                c[7].clone(),
            ]
        })
        .collect();
    let subtitle = format!(
        "{what} · {} rows · exported {}{}",
        rows.len(),
        widgets::local_time(&vf_core::now_rfc3339()),
        chain
            .map(|c| format!(" · {}", c.summary()))
            .unwrap_or_default()
    );
    vf_pdf::report::table("Vibe Files audit trail", &subtitle, &cols, &cells)
}

pub fn show(ui: &mut egui::Ui, p: &mut AuditPage, env: &mut Env, chain: Option<&ChainCheck>) {
    if p.rows.is_none() {
        p.load(env.conn);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Audit trail").strong().size(16.0));
        ui.label(widgets::caption(
            "Every change, hash-chained; also in _Vibe\\audit\\<month>.jsonl.",
        ));
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if let Some(c) = chain {
            let ok = c.result.as_ref().is_ok_and(|r| r.ok());
            ui.label(RichText::new(c.summary()).color(if ok {
                theme::INK_SECONDARY
            } else {
                ERROR
            }));
        }
        if ui
            .button("Verify now")
            .on_hover_text("Re-check every row's hash and its link to the row before")
            .clicked()
        {
            p.verify_requested = true;
        }
    });
    ui.add_space(6.0);

    // Filters.
    let before = p.filter.clone();
    ui.horizontal_wrapped(|ui| {
        let client_label = p
            .filter
            .client_id
            .as_ref()
            .and_then(|c| p.clients.iter().find(|(id, _)| id == c))
            .map_or("Any client".to_string(), |(_, n)| n.clone());
        let l = ui.label("Client");
        egui::ComboBox::from_id_salt("audit-client")
            .selected_text(client_label)
            .width(220.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut p.filter.client_id, None, "Any client");
                for (id, name) in &p.clients {
                    ui.selectable_value(&mut p.filter.client_id, Some(id.clone()), name);
                }
            })
            .response
            .labelled_by(l.id);
        let la = ui.label("Action");
        egui::ComboBox::from_id_salt("audit-action")
            .selected_text(p.filter.action.clone().unwrap_or_else(|| "Any".into()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut p.filter.action, None, "Any");
                for a in &p.actions {
                    ui.selectable_value(&mut p.filter.action, Some(a.clone()), a);
                }
            })
            .response
            .labelled_by(la.id);
        let l = ui.label("User");
        ui.add(
            TextEdit::singleline(&mut p.filter.user)
                .hint_text("any")
                .desired_width(110.0),
        )
        .labelled_by(l.id)
        .on_hover_text(p.users.join(", "));
        let l = ui.label("Document");
        ui.add(
            TextEdit::singleline(&mut p.filter.doc)
                .hint_text("part of the path")
                .desired_width(150.0),
        )
        .labelled_by(l.id);
        let l = ui.label("From");
        ui.add(
            TextEdit::singleline(&mut p.filter.from)
                .hint_text("yyyy-mm-dd")
                .desired_width(80.0),
        )
        .labelled_by(l.id);
        let l = ui.label("To");
        ui.add(
            TextEdit::singleline(&mut p.filter.to)
                .hint_text("yyyy-mm-dd")
                .desired_width(80.0),
        )
        .labelled_by(l.id);
        if ui.button("Clear").clicked() {
            p.filter = Filter::default();
        }
    });
    if p.filter != before {
        p.rows = None;
        p.load(env.conn);
    }
    let rows = p.rows.clone().unwrap_or_default();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(widgets::caption(if rows.len() == SHOWN {
            format!("The newest {SHOWN} matching rows (exports include every one).")
        } else {
            format!("{} rows.", rows.len())
        }));
        let export = |ext: &str| {
            rfd::FileDialog::new()
                .set_file_name(format!("Audit trail.{ext}"))
                .add_filter(ext.to_uppercase(), &[ext])
                .save_file()
        };
        if ui.button("Export CSV…").clicked()
            && let Some(dest) = export("csv")
        {
            let r = p.all(env.conn).and_then(|all| {
                std::fs::write(&dest, audit::to_csv(&all))
                    .map(|()| all.len())
                    .map_err(|e| e.to_string())
            });
            match r {
                Ok(n) => p.notice = Some(format!("Exported {n} rows to {}", dest.display())),
                Err(e) => p.error = Some(e),
            }
        }
        if ui.button("Export PDF…").clicked()
            && let Some(dest) = export("pdf")
        {
            let what = p.describe();
            let r = p.all(env.conn).and_then(|all| {
                std::fs::write(&dest, pdf(&all, &what, chain))
                    .map(|()| all.len())
                    .map_err(|e| e.to_string())
            });
            match r {
                Ok(n) => p.notice = Some(format!("Exported {n} rows to {}", dest.display())),
                Err(e) => p.error = Some(e),
            }
        }
    });
    if let Some(n) = &p.notice {
        ui.label(RichText::new(n).color(theme::INK_SECONDARY));
    }
    if let Some(e) = &p.error {
        ui.label(RichText::new(e).color(ERROR));
    }
    ui.separator();
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("audit-rows")
            .striped(true)
            .num_columns(7)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "#", "Time", "User", "Action", "Client", "Document", "Detail",
                ] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                for l in rows.iter().rev() {
                    let r = &l.row;
                    ui.label(RichText::new(r.seq.to_string()).monospace().small());
                    ui.label(widgets::local_time(&r.ts));
                    ui.label(r.user.as_deref().unwrap_or(&r.device));
                    ui.label(&r.action);
                    ui.label(l.client.as_deref().unwrap_or("—"));
                    ui.label(
                        RichText::new(l.doc_path.as_deref().unwrap_or("—"))
                            .monospace()
                            .small(),
                    );
                    let d: String = r.detail.chars().take(80).collect();
                    ui.label(RichText::new(d).small().color(theme::INK_MUTED))
                        .on_hover_text(&r.detail);
                    ui.end_row();
                }
            });
    });
}
