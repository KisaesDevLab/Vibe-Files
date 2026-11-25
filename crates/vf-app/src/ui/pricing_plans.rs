//! Admin › Portal › Pricing plans (ADDENDUM-tax-return-delivery.md §12.3, §15.6; T10, T12,
//! T18): a plan per return type and tax year: packages (each a percent of the client's return
//! fee with a minimum, its service lines, at most one Recommended) and add-ons. A delivery sends
//! the prices worked out for its client; a plan edit never changes a delivery already sent.

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::balance::{money, parse_money};
use vf_portal::pricing::{self, Plan, PlanAddon, PlanPackage, Service};

use crate::ui::{theme, widgets};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Tab {
    #[default]
    Packages,
    Addons,
}

#[derive(Default)]
pub struct PricingPlansPage {
    loaded: bool,
    plans: Vec<Plan>,
    draft: Option<Plan>,
    /// The grid's service rows (owner 2026-11-24, Q35): kept here so a row just added, with no
    /// bullets yet, stays until Save.
    rows: Vec<String>,
    tab: Tab,
    /// "Used by" (Q44): the deliveries sent with this plan's year and return type and a fee
    /// priced from a plan, read from the portal on a thread when the plan opens.
    used: Option<Result<Vec<UsedBy>, String>>,
    used_job: Option<std::sync::mpsc::Receiver<Result<Vec<UsedBy>, String>>>,
    /// A sample return fee for "As the client sees it".
    sample_fee: String,
    msg: Option<String>,
    err: Option<String>,
}

impl PricingPlansPage {
    pub fn reread(&mut self) {
        self.loaded = false;
    }

    /// The add-ons' names in their order (screens).
    #[cfg(test)]
    pub fn addon_names(&self) -> Vec<String> {
        self.draft
            .as_ref()
            .map(|d| d.addons.iter().map(|a| a.name.clone()).collect())
            .unwrap_or_default()
    }

    fn open(&mut self, plan: Plan) {
        self.used = None;
        self.used_job = None;
        self.rows = grid_rows(&plan.packages);
        self.draft = Some(plan);
        self.msg = None;
    }
}

/// A delivery a plan was used for: its client, title, status and fee total.
#[derive(Debug, Clone)]
pub struct UsedBy {
    pub client_id: String,
    pub title: String,
    pub status: String,
    pub total_cents: u64,
}

/// Asks the portal (on a thread) for the deliveries of a plan's year and return type whose
/// fee was priced from a plan (packages, or base fee and add-ons).
fn read_used(
    ctx: &egui::Context,
    used: &mut Option<Result<Vec<UsedBy>, String>>,
    job: &mut Option<std::sync::mpsc::Receiver<Result<Vec<UsedBy>, String>>>,
    root: &std::path::Path,
    plan: &Plan,
) {
    let Some(s) = crate::ui::portal::cabinet_id(root)
        .and_then(|id| vf_portal::settings::load(root, &id))
        .filter(|s| s.deliveries)
    else {
        *used = Some(Ok(Vec::new()));
        return;
    };
    let (year, rt) = (plan.tax_year, plan.return_type.clone());
    *job = Some(crate::ui::portal::spawn(ctx, move || {
        crate::portal::client(&s)
            .deliveries_for(year, &rt)
            .map(|ds| {
                ds.into_iter()
                    .filter_map(|d| {
                        let f = d.fee?;
                        matches!(f.mode.as_deref(), Some("packages" | "addons")).then(|| UsedBy {
                            client_id: d.client_id,
                            title: d.title,
                            status: d.status,
                            total_cents: f.total_cents,
                        })
                    })
                    .collect()
            })
            .map_err(|e| e.message())
    }));
}

/// Client names for "Used by", from the catalog.
fn client_name(conn: &vf_core::db::Connection, id: &str) -> String {
    conn.query_row(
        "SELECT name || ' (' || ext_id || ')' FROM client WHERE id = ?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_else(|_| "A client no longer in the list".into())
}

// ---- The packages grid: a column per package, a row per service (§15.6). A view over each
// package's `services` (group + bullets); nothing in the stored plan changes shape.

/// The grid's rows: every service group of the packages, in the order first met.
fn grid_rows(pkgs: &[PlanPackage]) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for p in pkgs {
        for s in &p.services {
            if !rows.contains(&s.group) {
                rows.push(s.group.clone());
            }
        }
    }
    rows
}

/// A package's cell for a row: its bullets, one a line.
fn cell(p: &PlanPackage, group: &str) -> String {
    p.services
        .iter()
        .filter(|s| s.group == group)
        .flat_map(|s| s.bullets.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Writes a cell back: an empty cell is no service for that package; the package's services
/// follow the rows' order.
fn set_cell(p: &mut PlanPackage, rows: &[String], group: &str, text: &str) {
    let bullets: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    p.services.retain(|s| s.group != group);
    if !bullets.is_empty() {
        p.services.push(Service {
            group: group.to_string(),
            bullets,
        });
    }
    order(p, rows);
}

fn order(p: &mut PlanPackage, rows: &[String]) {
    p.services.sort_by_key(|s| {
        rows.iter()
            .position(|r| *r == s.group)
            .unwrap_or(usize::MAX)
    });
}

/// A row renamed: every package's service of that group follows.
fn rename_row(pkgs: &mut [PlanPackage], from: &str, to: &str) {
    for p in pkgs {
        for s in p.services.iter_mut().filter(|s| s.group == from) {
            s.group = to.to_string();
        }
    }
}

fn this_year() -> i32 {
    use chrono::Datelike;
    chrono::Local::now().year()
}

pub fn page(ui: &mut egui::Ui, page: &mut PricingPlansPage, env: &mut crate::ui::browse::Env) {
    ui.heading("Pricing plans");
    if !page.loaded {
        page.loaded = true;
        page.plans = pricing::load_all(env.root);
        if page.sample_fee.is_empty() {
            page.sample_fee = "1,265.68".into();
        }
    }
    ui.label(widgets::caption(
        "The packages and add-ons a tax return delivery offers, per return type and tax year. A \
         package costs the client's return fee plus a percent of it, at least a minimum amount. \
         A delivery keeps the prices it was sent with: editing a plan changes new deliveries only.",
    ));
    if let Some(e) = &page.err {
        ui.colored_label(theme::error(), e);
    }
    if let Some(m) = &page.msg {
        ui.colored_label(theme::health_ok(), m);
    }
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(140.0);
            if ui.button("New plan…").clicked() {
                page.open(pricing::sample("1040", this_year() - 1));
            }
            ui.add_space(6.0);
            let mut pick = None;
            for p in &page.plans {
                let chosen = page
                    .draft
                    .as_ref()
                    .is_some_and(|d| d.return_type == p.return_type && d.tax_year == p.tax_year);
                if ui
                    .selectable_label(chosen, format!("{} {}", p.tax_year, p.return_type))
                    .clicked()
                {
                    pick = Some(p.clone());
                }
            }
            if let Some(p) = pick {
                page.open(p);
            }
        });
        ui.separator();
        ui.vertical(|ui| editor(ui, page, env));
    });
}

fn editor(ui: &mut egui::Ui, page: &mut PricingPlansPage, env: &mut crate::ui::browse::Env) {
    let Some(d) = &mut page.draft else {
        ui.label(widgets::caption(
            "Choose a plan on the left, or New plan… (it starts from the sample).",
        ));
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Return type");
        egui::ComboBox::from_id_salt("plan-type")
            .selected_text(&d.return_type)
            .show_ui(ui, |ui| {
                for rt in vf_portal::delivery::return_type_names(env.root) {
                    ui.selectable_value(&mut d.return_type, rt.clone(), rt);
                }
            });
        ui.label("Tax year");
        ui.add(egui::DragValue::new(&mut d.tax_year).range(1990..=2100));
        ui.label("Base fee name");
        ui.add(egui::TextEdit::singleline(&mut d.base_name).desired_width(200.0));
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut page.tab, Tab::Packages, "Packages");
        ui.selectable_value(&mut page.tab, Tab::Addons, "Add-ons");
    });
    ui.separator();
    // The grid (or the add-ons table) beside "As the client sees it".
    let side = 230.0;
    let main = (ui.available_width() - side - 24.0).max(320.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(main);
            match page.tab {
                Tab::Packages => packages_grid(ui, d, &mut page.rows),
                Tab::Addons => addons_table(ui, d),
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(side);
            ui.label(RichText::new("As the client sees it").strong());
            ui.horizontal(|ui| {
                ui.label("For a return fee of $");
                ui.add(egui::TextEdit::singleline(&mut page.sample_fee).desired_width(90.0));
            });
            if let Some(fee) = parse_money(&page.sample_fee) {
                for p in &d.packages {
                    ui.label(
                        RichText::new(format!(
                            "{}{}",
                            p.name,
                            if p.recommended { " · Recommended" } else { "" }
                        ))
                        .strong(),
                    );
                    ui.label(format!(
                        "{}  ({})",
                        money(pricing::package_price(fee, p)),
                        pricing::rule_words(p)
                    ));
                }
                let offered: Vec<&PlanAddon> = d.addons.iter().filter(|a| a.offered).collect();
                if !offered.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Add-ons").strong());
                    for a in offered {
                        ui.label(format!("{}: {}", a.name, money(a.cents)));
                    }
                }
            } else {
                ui.label(widgets::caption("Type an amount, e.g. 1,265.68."));
            }
            ui.separator();
            ui.label(RichText::new("Used by").strong());
            if let Some(rx) = &page.used_job
                && let Ok(r) = rx.try_recv()
            {
                page.used = Some(r);
                page.used_job = None;
            }
            if page.used.is_none() && page.used_job.is_none() {
                read_used(ui.ctx(), &mut page.used, &mut page.used_job, env.root, d);
            }
            match &page.used {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(widgets::caption("Reading the portal…"));
                    });
                }
                Some(Err(e)) => {
                    ui.colored_label(theme::error(), format!("Not read: {e}"));
                }
                Some(Ok(v)) if v.is_empty() => {
                    ui.label(widgets::caption(
                        "No delivery has been sent with this plan's prices.",
                    ));
                }
                Some(Ok(v)) => {
                    ui.label(widgets::caption(format!(
                        "{} {} sent with this plan's prices. They keep the prices they were \
                         sent with: a change here is for new deliveries only.",
                        v.len(),
                        if v.len() == 1 {
                            "delivery was"
                        } else {
                            "deliveries were"
                        }
                    )));
                    for u in v.iter().take(12) {
                        ui.label(format!(
                            "{}: {}, {}",
                            client_name(env.conn, &u.client_id),
                            money(u.total_cents),
                            u.status
                        ))
                        .on_hover_text(&u.title);
                    }
                    if v.len() > 12 {
                        ui.label(widgets::caption(format!("and {} more", v.len() - 12)));
                    }
                }
            }
            if ui.small_button("Read again").clicked() {
                page.used = None;
            }
        });
    });
    ui.separator();
    let mut save = false;
    let mut copy_next = false;
    let mut delete = false;
    ui.horizontal(|ui| {
        if ui.add(widgets::primary_button("Save")).clicked() {
            save = true;
        }
        if ui.button("Copy to next year…").clicked() {
            copy_next = true;
        }
        if ui
            .button(RichText::new("Delete").color(theme::error()))
            .clicked()
        {
            delete = true;
        }
    });
    let plan = d.clone();
    let audit = |env: &mut crate::ui::browse::Env, action: &str, p: &Plan| {
        crate::ui::portal::record(
            env,
            action,
            vec![AuditEntry::new(
                action,
                None,
                serde_json::json!({
                    "return_type": p.return_type,
                    "tax_year": p.tax_year,
                    "fields": ["packages", "addons", "base_name"],
                }),
            )],
        )
        .unwrap_or_default()
    };
    if save {
        match pricing::save(env.root, &plan) {
            Ok(()) => {
                let u = audit(env, "pricing_plan_save", &plan);
                page.msg = Some(format!(
                    "Saved the {} {} plan.{u}",
                    plan.tax_year, plan.return_type
                ));
                page.err = None;
                page.loaded = false;
            }
            Err(e) => page.err = Some(e),
        }
    }
    if copy_next {
        let mut next = plan.clone();
        next.tax_year += 1;
        let next_year = next.tax_year;
        if pricing::load(env.root, &next.return_type, next.tax_year).is_some() {
            page.err = Some(format!(
                "A {} {} plan exists already.",
                next.tax_year, next.return_type
            ));
        } else {
            match pricing::save(env.root, &next) {
                Ok(()) => {
                    let u = audit(env, "pricing_plan_save", &next);
                    page.open(next);
                    page.msg = Some(format!("Copied to {}.{u}", next_year));
                    page.loaded = false;
                }
                Err(e) => page.err = Some(e),
            }
        }
    }
    if delete {
        match pricing::delete(env.root, &plan.return_type, plan.tax_year) {
            Ok(()) => {
                let u = audit(env, "pricing_plan_delete", &plan);
                page.msg = Some(format!("Deleted.{u}"));
                page.draft = None;
                page.loaded = false;
            }
            Err(e) => page.err = Some(e),
        }
    }
}

/// Packages: the first rows are the package's name, its price over the return fee and
/// Recommended; then a row per service whose cells hold each package's bullets, one a line.
fn packages_grid(ui: &mut egui::Ui, d: &mut Plan, rows: &mut Vec<String>) {
    let mut rec: Option<usize> = d.packages.iter().position(|p| p.recommended);
    let mut remove_pkg = None;
    let mut remove_row = None;
    let mut rename: Option<(usize, String)> = None;
    let col = 160.0;
    egui::ScrollArea::horizontal()
        .id_salt("plan-grid-scroll")
        .show(ui, |ui| {
            egui::Grid::new("plan-grid")
                .num_columns(d.packages.len() + 1)
                .striped(true)
                .spacing([10.0, 8.0])
                .min_col_width(col)
                .show(ui, |ui| {
                    ui.label(RichText::new("Package").strong());
                    for p in d.packages.iter_mut() {
                        widgets::named(
                            ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(col)),
                            "Package name",
                        );
                    }
                    ui.end_row();
                    ui.label("Price over the return fee");
                    for (i, p) in d.packages.iter_mut().enumerate() {
                        if i == 0 {
                            ui.label(widgets::caption("the return fee itself"));
                            continue;
                        }
                        ui.vertical(|ui| {
                            let mut pct = f64::from(p.percent_bp) / 100.0;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut pct)
                                        .range(0.0..=500.0)
                                        .speed(0.5)
                                        .suffix(" %"),
                                )
                                .changed()
                            {
                                p.percent_bp = (pct * 100.0).round() as u32;
                            }
                            ui.horizontal(|ui| {
                                ui.label("at least $");
                                let mut min =
                                    money(p.minimum_cents).trim_start_matches('$').to_string();
                                if ui
                                    .add(egui::TextEdit::singleline(&mut min).desired_width(70.0))
                                    .changed()
                                    && let Some(c) = parse_money(&min)
                                {
                                    p.minimum_cents = c;
                                }
                            });
                        });
                    }
                    ui.end_row();
                    ui.label("Recommended");
                    for i in 0..d.packages.len() {
                        ui.radio_value(&mut rec, Some(i), "Recommended");
                    }
                    ui.end_row();
                    for (r, row) in rows.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let mut name = row.clone();
                            if widgets::named(
                                ui.add(
                                    egui::TextEdit::singleline(&mut name)
                                        .desired_width(col - 60.0)
                                        .hint_text("Service"),
                                ),
                                "Service",
                            )
                            .changed()
                            {
                                rename = Some((r, name));
                            }
                            if ui.small_button("Remove").clicked() {
                                remove_row = Some(r);
                            }
                        });
                        for p in d.packages.iter_mut() {
                            let mut text = cell(p, row);
                            if ui
                                .add(
                                    egui::TextEdit::multiline(&mut text)
                                        .desired_rows(2)
                                        .desired_width(col)
                                        .hint_text("A bullet a line"),
                                )
                                .changed()
                            {
                                set_cell(p, rows, row, &text);
                            }
                        }
                        ui.end_row();
                    }
                    ui.label("");
                    for (i, _) in d.packages.iter().enumerate() {
                        if ui
                            .add_enabled(i > 0, egui::Button::new("Remove package").small())
                            .clicked()
                        {
                            remove_pkg = Some(i);
                        }
                    }
                    ui.end_row();
                });
        });
    for (i, p) in d.packages.iter_mut().enumerate() {
        p.recommended = rec == Some(i);
    }
    if let Some((r, to)) = rename {
        let from = rows[r].clone();
        // A name another row has would merge the two: kept apart until it differs.
        if !rows.iter().enumerate().any(|(j, x)| j != r && *x == to) {
            rename_row(&mut d.packages, &from, &to);
            rows[r] = to;
        }
    }
    if let Some(r) = remove_row {
        let g = rows.remove(r);
        for p in d.packages.iter_mut() {
            p.services.retain(|s| s.group != g);
        }
    }
    if let Some(i) = remove_pkg {
        d.packages.remove(i);
    }
    ui.horizontal(|ui| {
        if ui.button("Add package").clicked() {
            d.packages.push(PlanPackage {
                key: format!("pkg-{}", &vf_core::new_id()[..8]),
                name: "New package".into(),
                recommended: false,
                percent_bp: 0,
                minimum_cents: 0,
                services: vec![],
            });
        }
        if ui.button("Add service").clicked() {
            let mut n = rows.len() + 1;
            while rows.contains(&format!("Service {n}")) {
                n += 1;
            }
            rows.push(format!("Service {n}"));
        }
    });
    ui.label(widgets::caption(
        "A cell holds the lines the client compares for that package, one a line; an empty cell \
         is not shown for that package. The first package is the return fee itself.",
    ));
}

/// An item moved from one place in a list to another.
fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if from < v.len() && to < v.len() && from != to {
        let x = v.remove(from);
        v.insert(to, x);
    }
}

/// Add-ons: a drag handle (Q43), Name, What the client reads, Price, Offered unless unticked.
fn addons_table(ui: &mut egui::Ui, d: &mut Plan) {
    let mut remove_add = None;
    let mut row_y: Vec<f32> = Vec::new();
    egui::Grid::new("plan-addons")
        .num_columns(6)
        .striped(true)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("What the client reads").strong());
            ui.label(RichText::new("Price").strong());
            ui.label(RichText::new("Offered unless unticked").strong());
            ui.label("");
            ui.end_row();
            for (i, a) in d.addons.iter_mut().enumerate() {
                // The handle: three short lines, dragged to move the add-on.
                let r = ui
                    .dnd_drag_source(egui::Id::unique(("plan-addon-drag", i)), i, |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 18.0), egui::Sense::hover());
                        let c = theme::ink_muted();
                        for k in 0..3 {
                            let y = rect.top() + 5.0 + k as f32 * 4.0;
                            ui.painter().line_segment(
                                [
                                    egui::pos2(rect.left() + 2.0, y),
                                    egui::pos2(rect.right() - 2.0, y),
                                ],
                                egui::Stroke::new(1.5, c),
                            );
                        }
                    })
                    .response;
                row_y.push(r.rect.center().y);
                widgets::named(r.on_hover_text("Drag to move"), "Move add-on");
                ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(160.0));
                ui.add(
                    egui::TextEdit::singleline(&mut a.text)
                        .desired_width(260.0)
                        .hint_text("What the client reads"),
                );
                ui.horizontal(|ui| {
                    ui.label("$");
                    let mut price = money(a.cents).trim_start_matches('$').to_string();
                    if ui
                        .add(egui::TextEdit::singleline(&mut price).desired_width(70.0))
                        .changed()
                        && let Some(c) = parse_money(&price)
                    {
                        a.cents = c;
                    }
                });
                ui.checkbox(&mut a.offered, "");
                if ui.small_button("Remove").clicked() {
                    remove_add = Some(i);
                }
                ui.end_row();
            }
        });
    // Dropped: the add-on goes to the row nearest the pointer.
    if let Some(from) = egui::DragAndDrop::payload::<usize>(ui.ctx())
        && ui.input(|i| i.pointer.any_released())
    {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos())
            && let Some(to) = row_y
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - p.y).abs().total_cmp(&(b.1 - p.y).abs()))
                .map(|x| x.0)
        {
            move_item(&mut d.addons, *from, to);
        }
        egui::DragAndDrop::clear_payload(ui.ctx());
    }
    if let Some(i) = remove_add {
        d.addons.remove(i);
    }
    if ui.button("Add add-on").clicked() {
        d.addons.push(PlanAddon {
            key: format!("add-{}", &vf_core::new_id()[..8]),
            name: "New add-on".into(),
            text: String::new(),
            cents: 0,
            offered: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(services: &[(&str, &[&str])]) -> PlanPackage {
        PlanPackage {
            key: "k".into(),
            name: "P".into(),
            recommended: false,
            percent_bp: 0,
            minimum_cents: 0,
            services: services
                .iter()
                .map(|(g, b)| Service {
                    group: g.to_string(),
                    bullets: b.iter().map(|x| x.to_string()).collect(),
                })
                .collect(),
        }
    }

    /// The grid is a view: reading every cell and writing it back changes nothing, a service
    /// one package lacks stays absent, and names with `:` or `;` survive.
    #[test]
    fn the_grid_round_trips() {
        let mut plan = pricing::sample("1040", 2025);
        plan.packages[1].services.push(Service {
            group: "Notices: IRS; state".into(),
            bullets: vec!["Letters: answered; filed".into()],
        });
        let before = plan.packages.clone();
        let rows = grid_rows(&plan.packages);
        assert!(rows.contains(&"Notices: IRS; state".to_string()));
        for p in plan.packages.iter_mut() {
            for r in &rows {
                let c = cell(p, r);
                set_cell(p, &rows, r, &c);
            }
        }
        assert_eq!(plan.packages, before);
        assert_eq!(cell(&plan.packages[0], "Notices: IRS; state"), "");
    }

    #[test]
    fn add_ons_move_where_dropped() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        move_item(&mut v, 0, 2);
        assert_eq!(v, ['b', 'c', 'a', 'd']);
        move_item(&mut v, 3, 0);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        move_item(&mut v, 1, 9);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
    }

    #[test]
    fn cells_rows_and_renames() {
        let mut a = pkg(&[("Return", &["Preparation"])]);
        let mut b = pkg(&[("Return", &["Preparation"]), ("Audit", &["Defense"])]);
        let mut rows = grid_rows(&[a.clone(), b.clone()]);
        assert_eq!(rows, ["Return", "Audit"]);
        // A new row filled for one package lands in row order.
        rows.insert(1, "Planning".into());
        set_cell(&mut b, &rows, "Planning", "Meeting\n\n Projection ");
        assert_eq!(
            b.services
                .iter()
                .map(|s| s.group.as_str())
                .collect::<Vec<_>>(),
            ["Return", "Planning", "Audit"]
        );
        assert_eq!(cell(&b, "Planning"), "Meeting\nProjection");
        // Emptied: no service.
        set_cell(&mut b, &rows, "Audit", "  ");
        assert!(b.services.iter().all(|s| s.group != "Audit"));
        let mut both = [a.clone(), b.clone()];
        rename_row(&mut both, "Return", "Tax return");
        assert!(both.iter().all(|p| p.services[0].group == "Tax return"));
        a = both[0].clone();
        assert_eq!(cell(&a, "Tax return"), "Preparation");
    }
}
