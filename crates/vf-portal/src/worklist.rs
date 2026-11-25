//! Admin › Portal › Deliveries, the season's worklist of tax return deliveries and annual
//! agreements (ADDENDUM-annual-agreements.md §12, owner 2026-11-25): which rows each filter
//! shows, the season a finished one belongs to, and the firm's "waiting too long" mark,
//! `<cabinet>/_Vibe/deliveries.json` (one number for every PC).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agreement::Subscription;
use crate::delivery::Delivery;

pub const FILE: &str = "deliveries.json";

/// The mark when the firm has set none.
pub const OVERDUE_DAYS: u32 = 14;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    overdue_days: u32,
}

fn path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(FILE)
}

/// After how many days an open item is marked as waiting too long (14 unless the firm set it).
pub fn overdue_days(root: &Path) -> u32 {
    std::fs::read(path(root))
        .ok()
        .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
        .map(|s| s.overdue_days)
        .filter(|d| *d > 0)
        .unwrap_or(OVERDUE_DAYS)
}

/// Saves the firm's mark (1 to 365 days).
pub fn set_overdue_days(root: &Path, days: u32) -> Result<u32, String> {
    let days = days.clamp(1, 365);
    vf_fs::lock::with(root, "deliveries", "the deliveries page settings", || {
        let bytes = serde_json::to_vec_pretty(&Saved { overdue_days: days }).expect("serializes");
        vf_fs::cabinet::write_replace(&path(root), &bytes)
    })?;
    Ok(days)
}

/// What the page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    /// Every open delivery and agreement.
    #[default]
    Open,
    /// Those waiting on this step (`delivery::RETURN`, `ESIGN`, `LETTERS`, `PACKAGE`, `FEE`).
    Step(&'static str),
    /// Tax returns only.
    Returns,
    /// Annual agreements only.
    Agreements,
    /// Clients whose last monthly charge did not go through: their agreement, open or not.
    Failed,
}

/// The season a delivery belongs to: a tax return's tax year; an agreement's the tax year of
/// the returns it covers (the 2026 agreement covers the 2025 returns, owner A11).
pub fn season_of(d: &Delivery) -> i32 {
    if d.is_agreement() {
        d.return_year.unwrap_or(d.tax_year - 1)
    } else {
        d.tax_year
    }
}

/// One row of the page.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    pub d: &'a Delivery,
    /// The client's live monthly charge, on an agreement's row.
    pub monthly: Option<&'a Subscription>,
}

impl Row<'_> {
    pub fn open(&self) -> bool {
        self.d.status == "open"
    }

    /// Open and waiting `days` or more since it was sent.
    pub fn overdue(&self, days_open: i64, days: u32) -> bool {
        self.open() && days_open >= i64::from(days)
    }
}

/// The rows for `show`: open ones (the portal's order, newest first), then, when `season` is
/// given, those finished in that season. Cancelled ones never.
pub fn rows<'a>(
    list: &'a [Delivery],
    subs: &'a [Subscription],
    show: Show,
    season: Option<i32>,
) -> Vec<Row<'a>> {
    let monthly = |d: &Delivery| {
        d.is_agreement()
            .then(|| subs.iter().find(|s| s.client_id == d.client_id && s.live()))
            .flatten()
    };
    let row = |d: &'a Delivery| Row {
        d,
        monthly: monthly(d),
    };
    if show == Show::Failed {
        // The agreement that set the charge, else the client's newest.
        return subs
            .iter()
            .filter(|s| s.live() && s.failed_at.is_some())
            .filter_map(|s| {
                let mine = || {
                    list.iter()
                        .filter(|d| d.client_id == s.client_id && d.is_agreement())
                        .filter(|d| d.status != "cancelled")
                };
                mine()
                    .find(|d| d.id == s.delivery_id)
                    .or_else(|| mine().max_by_key(|d| d.tax_year))
            })
            .map(row)
            .collect();
    }
    let wanted = |d: &Delivery| match show {
        Show::Open | Show::Failed => true,
        Show::Returns => !d.is_agreement(),
        Show::Agreements => d.is_agreement(),
        Show::Step(k) => {
            if d.status == "open" {
                d.waiting().is_some_and(|s| s.kind == k)
            } else {
                d.step(k).is_some()
            }
        }
    };
    let open = list.iter().filter(|d| d.status == "open");
    let finished = list
        .iter()
        .filter(|d| d.status == "complete" && season.is_some_and(|y| season_of(d) == y));
    open.chain(finished)
        .filter(|d| wanted(d))
        .map(row)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: &str, client: &str, kind: &str, year: i32, status: &str, waiting: &str) -> Delivery {
        let step = |k: &str, state: &str| {
            serde_json::json!({
                "kind": k, "n": 1, "name": k, "present": true, "state": state,
                "saidAt": null, "completedAt": null, "completedBy": null
            })
        };
        let steps: Vec<_> = ["letters", "return", "fee"]
            .iter()
            .map(|k| {
                let state = if status == "open" && *k == waiting {
                    "open"
                } else {
                    "done"
                };
                step(k, state)
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": id, "clientId": client, "taxYear": year, "returnType": "1040", "kind": kind,
            "title": "t", "status": status,
            "locks": {"returnAfterLetters": false, "esignAfterLetters": false, "esignAfterFee": false},
            "assignedStaffId": null, "remindedAt": null, "returnFile": null, "esignLink": null,
            "createdAt": "2026-11-01T00:00:00Z", "completedAt": null, "cancelledAt": null,
            "steps": steps
        }))
        .unwrap()
    }

    fn sub(client: &str, delivery: &str, failed: bool) -> Subscription {
        serde_json::from_value(serde_json::json!({
            "id": "s", "clientId": client, "method": "card", "monthlyCents": 12000,
            "status": "active", "failedAt": failed.then_some("2026-11-01T00:00:00Z"),
            "canceledAt": null, "deliveryId": delivery, "createdAt": "2026-10-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn ids(r: &[Row]) -> Vec<String> {
        r.iter().map(|r| r.d.id.clone()).collect()
    }

    #[test]
    fn each_filter_lists_its_rows() {
        let list = vec![
            d("r1", "c1", "return", 2025, "open", "fee"),
            d("a1", "c2", "agreement", 2026, "open", "letters"),
            d("r2", "c3", "return", 2025, "complete", ""),
            d("r3", "c4", "return", 2024, "complete", ""),
            d("a2", "c5", "agreement", 2026, "complete", ""),
            d("x", "c6", "return", 2025, "cancelled", ""),
        ];
        let subs = vec![sub("c5", "a2", true), sub("c2", "a1", false)];
        assert_eq!(ids(&rows(&list, &subs, Show::Open, None)), ["r1", "a1"]);
        // This season (2025): the 2025 return and the 2026 agreement, not 2024's.
        assert_eq!(
            ids(&rows(&list, &subs, Show::Open, Some(2025))),
            ["r1", "a1", "r2", "a2"]
        );
        assert_eq!(ids(&rows(&list, &subs, Show::Step("fee"), None)), ["r1"]);
        assert_eq!(
            ids(&rows(&list, &subs, Show::Step("letters"), None)),
            ["a1"]
        );
        assert_eq!(
            ids(&rows(&list, &subs, Show::Returns, Some(2025))),
            ["r1", "r2"]
        );
        assert_eq!(ids(&rows(&list, &subs, Show::Agreements, None)), ["a1"]);
        // A failed month: the finished agreement that set the charge, season or not.
        let failed = rows(&list, &subs, Show::Failed, None);
        assert_eq!(ids(&failed), ["a2"]);
        assert!(failed[0].monthly.is_some_and(|s| s.failed_at.is_some()));
        // The monthly charge shows on agreements only.
        let all = rows(&list, &subs, Show::Open, None);
        assert!(all[0].monthly.is_none() && all[1].monthly.is_some());
    }

    #[test]
    fn the_firms_mark_is_saved_for_the_cabinet() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(vf_fs::cabinet::VIBE_DIR)).unwrap();
        assert_eq!(overdue_days(t.path()), OVERDUE_DAYS);
        assert_eq!(set_overdue_days(t.path(), 21).unwrap(), 21);
        assert_eq!(overdue_days(t.path()), 21);
        assert_eq!(set_overdue_days(t.path(), 0).unwrap(), 1);
    }
}
