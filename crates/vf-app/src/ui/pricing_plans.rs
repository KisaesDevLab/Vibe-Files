//! Admin › Portal › Pricing plans (ADDENDUM-tax-return-delivery.md §12.3, §15.6; T10, T12,
//! T18): a plan per return type and tax year: packages (each a percent of the client's return
//! fee with a minimum, its service lines, at most one Recommended) and add-ons. A delivery sends
//! the prices worked out for its client; a plan edit never changes a delivery already sent.

use egui::RichText;
use vf_core::audit::AuditEntry;
use vf_portal::balance::{money, parse_money};
use vf_portal::pricing::{self, Plan, PlanAddon, PlanPackage, Service};

use crate::ui::{theme, widgets};

#[derive(Default)]
pub struct PricingPlansPage {
    loaded: bool,
    plans: Vec<Plan>,
    draft: Option<Plan>,
    /// A sample return fee for "As the client sees it".
    sample_fee: String,
    msg: Option<String>,
    err: Option<String>,
}

impl PricingPlansPage {
    pub fn reread(&mut self) {
        self.loaded = false;
    }
}

/// A package's services as lines: `Group: bullet; bullet`.
fn services_text(s: &[Service]) -> String {
    s.iter()
        .map(|x| format!("{}: {}", x.group, x.bullets.join("; ")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn services_of(text: &str) -> Vec<Service> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (g, b) = l.split_once(':').unwrap_or((l, ""));
            Service {
                group: g.trim().to_string(),
                bullets: b
                    .split(';')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(String::from)
                    .collect(),
            }
        })
        .collect()
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
            ui.set_width(220.0);
            if ui.button("New plan…").clicked() {
                page.draft = Some(pricing::sample("1040", this_year() - 1));
                page.msg = None;
            }
            ui.add_space(6.0);
            for p in &page.plans {
                let chosen = page
                    .draft
                    .as_ref()
                    .is_some_and(|d| d.return_type == p.return_type && d.tax_year == p.tax_year);
                if ui
                    .selectable_label(chosen, format!("{} {}", p.tax_year, p.return_type))
                    .clicked()
                {
                    page.draft = Some(p.clone());
                    page.msg = None;
                }
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
                for rt in vf_portal::delivery::RETURN_TYPES {
                    ui.selectable_value(&mut d.return_type, rt.to_string(), rt);
                }
            });
        ui.label("Tax year");
        ui.add(egui::DragValue::new(&mut d.tax_year).range(1990..=2100));
        ui.label("Base fee name");
        ui.add(egui::TextEdit::singleline(&mut d.base_name).desired_width(200.0));
    });
    ui.separator();
    ui.label(RichText::new("PACKAGES").small().color(theme::ink_muted()));
    let mut remove_pkg = None;
    let mut rec: Option<usize> = d.packages.iter().position(|p| p.recommended);
    for (i, p) in d.packages.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                widgets::named(
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(160.0)),
                    "Package name",
                );
                ui.label("over the return fee:");
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
                ui.label("at least $");
                let mut min = money(p.minimum_cents).trim_start_matches('$').to_string();
                if ui
                    .add(egui::TextEdit::singleline(&mut min).desired_width(80.0))
                    .changed()
                    && let Some(c) = parse_money(&min)
                {
                    p.minimum_cents = c;
                }
                ui.radio_value(&mut rec, Some(i), "Recommended");
                if ui.small_button("Remove").clicked() {
                    remove_pkg = Some(i);
                }
            });
            let mut svc = services_text(&p.services);
            if ui
                .add(
                    egui::TextEdit::multiline(&mut svc)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Group: bullet; bullet (a group a line)"),
                )
                .changed()
            {
                p.services = services_of(&svc);
            }
        });
    }
    for (i, p) in d.packages.iter_mut().enumerate() {
        p.recommended = rec == Some(i);
    }
    if let Some(i) = remove_pkg {
        d.packages.remove(i);
    }
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
    ui.separator();
    ui.label(RichText::new("ADD-ONS").small().color(theme::ink_muted()));
    let mut remove_add = None;
    for (i, a) in d.addons.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(180.0));
            ui.add(
                egui::TextEdit::singleline(&mut a.text)
                    .desired_width(260.0)
                    .hint_text("What the client reads"),
            );
            ui.label("$");
            let mut price = money(a.cents).trim_start_matches('$').to_string();
            if ui
                .add(egui::TextEdit::singleline(&mut price).desired_width(80.0))
                .changed()
                && let Some(c) = parse_money(&price)
            {
                a.cents = c;
            }
            ui.checkbox(&mut a.offered, "Offered unless unticked");
            if ui.small_button("Remove").clicked() {
                remove_add = Some(i);
            }
        });
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
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("As the client sees it, for a return fee of $");
        ui.add(egui::TextEdit::singleline(&mut page.sample_fee).desired_width(90.0));
    });
    if let Some(fee) = parse_money(&page.sample_fee) {
        for p in &d.packages {
            ui.label(format!(
                "{}{}: {}  ({})",
                p.name,
                if p.recommended { " · Recommended" } else { "" },
                money(pricing::package_price(fee, p)),
                pricing::rule_words(p)
            ));
        }
    }
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
        if pricing::load(env.root, &next.return_type, next.tax_year).is_some() {
            page.err = Some(format!(
                "A {} {} plan exists already.",
                next.tax_year, next.return_type
            ));
        } else {
            match pricing::save(env.root, &next) {
                Ok(()) => {
                    let u = audit(env, "pricing_plan_save", &next);
                    page.msg = Some(format!("Copied to {}.{u}", next.tax_year));
                    page.draft = Some(next);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_edit_as_lines() {
        let s = services_of("Return: Preparation; e-filing\n\nNotices: IRS letters");
        assert_eq!(s.len(), 2);
        assert_eq!(
            services_text(&s),
            "Return: Preparation; e-filing\nNotices: IRS letters"
        );
    }
}
