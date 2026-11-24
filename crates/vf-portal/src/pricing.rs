//! Pricing plans (ADDENDUM-tax-return-delivery.md §12.3, owner 2026-11-24; T10, T12, T18, T29):
//! a plan per return type and tax year holds the packages a delivery offers (each priced as a
//! percent of the client's return fee with a minimum) and the add-ons. A delivery sends a priced
//! snapshot worked out from the plan for its client; editing a plan never changes a delivery
//! already sent. Kept in the cabinet: `_Vibe/pricing/<return type> <year>.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DIR: &str = "pricing";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    pub group: String,
    pub bullets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPackage {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub recommended: bool,
    /// Over the return fee, in basis points (10 % = 1000).
    #[serde(default)]
    pub percent_bp: u32,
    /// At least this much over the return fee, in cents.
    #[serde(default)]
    pub minimum_cents: u64,
    #[serde(default)]
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAddon {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub text: String,
    pub cents: u64,
    /// Offered unless the delivery unticks it.
    #[serde(default = "yes")]
    pub offered: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub return_type: String,
    pub tax_year: i32,
    /// The base fee's name (base fee and add-ons mode).
    #[serde(default = "base_name")]
    pub base_name: String,
    /// The first is the return fee itself (no percent, no minimum).
    pub packages: Vec<PlanPackage>,
    #[serde(default)]
    pub addons: Vec<PlanAddon>,
}

fn base_name() -> String {
    "Tax return preparation".into()
}

/// A package's price for a return fee: `return_fee + max(return_fee × percent, minimum)`, the
/// percent's amount rounded half up to the cent (T18).
pub fn package_price(return_fee_cents: u64, p: &PlanPackage) -> u64 {
    let over = (return_fee_cents * u64::from(p.percent_bp) + 5_000) / 10_000;
    return_fee_cents + over.max(p.minimum_cents)
}

/// "10 % (at least $127.00)".
pub fn rule_words(p: &PlanPackage) -> String {
    if p.percent_bp == 0 && p.minimum_cents == 0 {
        return "the return fee".into();
    }
    let pct = f64::from(p.percent_bp) / 100.0;
    format!(
        "{} % (at least {})",
        if pct.fract() == 0.0 {
            format!("{pct:.0}")
        } else {
            format!("{pct}")
        },
        crate::balance::money(p.minimum_cents)
    )
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(DIR)
}

fn path(root: &Path, return_type: &str, year: i32) -> PathBuf {
    let rt: String = return_type
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    dir(root).join(format!("{rt} {year}.json"))
}

pub fn load_all(root: &Path) -> Vec<Plan> {
    let Ok(rd) = std::fs::read_dir(dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<Plan> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by(|a, b| (b.tax_year, &a.return_type).cmp(&(a.tax_year, &b.return_type)));
    out
}

pub fn load(root: &Path, return_type: &str, year: i32) -> Option<Plan> {
    serde_json::from_str(&std::fs::read_to_string(path(root, return_type, year)).ok()?).ok()
}

pub fn save(root: &Path, plan: &Plan) -> Result<(), String> {
    if plan.packages.iter().filter(|p| p.recommended).count() > 1 {
        return Err("Mark at most one package Recommended.".into());
    }
    std::fs::create_dir_all(dir(root)).map_err(|e| e.to_string())?;
    vf_fs::cabinet::write_replace(
        &path(root, &plan.return_type, plan.tax_year),
        &serde_json::to_vec_pretty(plan).expect("serializes"),
    )
}

pub fn delete(root: &Path, return_type: &str, year: i32) -> Result<(), String> {
    std::fs::remove_file(path(root, return_type, year)).map_err(|e| e.to_string())
}

/// The sample plan (§12.3): Tax Return Only, Advantage 10 % (at least $127), Concierge 35 % (at
/// least $443), and two add-ons. Marked by its names; the firm edits it.
pub fn sample(return_type: &str, year: i32) -> Plan {
    let svc = |group: &str, bullets: &[&str]| Service {
        group: group.into(),
        bullets: bullets.iter().map(|b| b.to_string()).collect(),
    };
    Plan {
        return_type: return_type.into(),
        tax_year: year,
        base_name: base_name(),
        packages: vec![
            PlanPackage {
                key: "pkg-return".into(),
                name: "Tax Return Only".into(),
                recommended: false,
                percent_bp: 0,
                minimum_cents: 0,
                services: vec![svc("Return", &["Preparation and e-filing"])],
            },
            PlanPackage {
                key: "pkg-advantage".into(),
                name: "Advantage".into(),
                recommended: true,
                percent_bp: 1000,
                minimum_cents: 12_700,
                services: vec![
                    svc("Return", &["Preparation and e-filing"]),
                    svc("Notices", &["IRS and state letters answered"]),
                ],
            },
            PlanPackage {
                key: "pkg-concierge".into(),
                name: "Concierge".into(),
                recommended: false,
                percent_bp: 3500,
                minimum_cents: 44_300,
                services: vec![
                    svc("Return", &["Preparation and e-filing"]),
                    svc(
                        "Notices",
                        &["IRS and state letters answered", "Audit representation"],
                    ),
                    svc("Planning", &["Mid-year planning meeting"]),
                ],
            },
        ],
        addons: vec![
            PlanAddon {
                key: "add-planning".into(),
                name: "Mid-year planning meeting".into(),
                text: "A meeting in the summer to plan for next year's return.".into(),
                cents: 25_000,
                offered: true,
            },
            PlanAddon {
                key: "add-estimates".into(),
                name: "Quarterly estimate review".into(),
                text: "Your estimated payments checked each quarter.".into(),
                cents: 15_000,
                offered: true,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §12.3 / F3's gate: on a $1,265.68 return fee both minimums apply.
    #[test]
    fn the_sample_plan_gives_the_addendums_prices() {
        let p = sample("1040", 2025);
        let fee = 126_568;
        let prices: Vec<u64> = p.packages.iter().map(|k| package_price(fee, k)).collect();
        assert_eq!(prices, [126_568, 139_268, 170_868]);
        // Above the minimums the percent applies, rounded half up to the cent.
        assert_eq!(package_price(200_005, &p.packages[1]), 200_005 + 20_001);
        assert_eq!(rule_words(&p.packages[1]), "10 % (at least $127.00)");
        assert_eq!(rule_words(&p.packages[0]), "the return fee");
    }

    #[test]
    fn plans_are_kept_per_return_type_and_year() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &sample("1040", 2025)).unwrap();
        let mut next = sample("1040", 2025);
        next.tax_year = 2026;
        next.packages[1].minimum_cents = 13_000;
        save(t.path(), &next).unwrap();
        assert_eq!(
            load(t.path(), "1040", 2025).unwrap().packages[1].minimum_cents,
            12_700
        );
        assert_eq!(load_all(t.path()).len(), 2);
        let mut two = sample("1040", 2025);
        two.packages[0].recommended = true;
        assert!(save(t.path(), &two).is_err());
    }
}
