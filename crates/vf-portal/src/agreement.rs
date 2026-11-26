//! Annual service agreements (ADDENDUM-annual-agreements.md, owner 2026-11-25; Vibe Portal
//! Addendum 03): a delivery of kind `agreement` (letters, an optional package, the fee) whose fee
//! is a line per service, charged once or every month (A10). The monthly lines become a monthly
//! charge on the firm's Stripe account that runs until cancelled (A9); a renewal changes it from
//! the next charge (A13). Staff change or stop it; End engagement… stops it and cancels the
//! client's open agreements (A12).

use serde::{Deserialize, Serialize};

use crate::api::{Portal, Result};
use crate::balance::money;
use crate::delivery::FeeLineIn;

/// The return type an agreement has on the portal (one open agreement per client and year).
pub const AGREEMENT: &str = "Agreement";

/// A client's monthly charge as the portal has it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub client_id: String,
    /// `card` or `bank`.
    pub method: String,
    pub monthly_cents: u64,
    #[serde(default)]
    pub card_fee_cents: u64,
    /// Stripe's status: `active`, `past_due`, `canceled`…
    pub status: String,
    pub failed_at: Option<String>,
    pub canceled_at: Option<String>,
    /// The agreement that set its amount.
    pub delivery_id: String,
    pub created_at: String,
}

impl Subscription {
    pub fn live(&self) -> bool {
        self.canceled_at.is_none()
    }

    /// What the client pays a month (with the card fee on a card).
    pub fn total_cents(&self) -> u64 {
        self.monthly_cents
            + if self.method == "card" {
                self.card_fee_cents
            } else {
                0
            }
    }

    /// "$450.00 a month by card", "…: the last payment failed", "Ended".
    pub fn words(&self) -> String {
        if !self.live() {
            return "Monthly charge ended".into();
        }
        let how = if self.method == "bank" {
            "bank account"
        } else {
            "card"
        };
        let base = format!("{} a month by {how}", money(self.total_cents()));
        if self.failed_at.is_some() {
            format!("{base}: the last payment failed")
        } else {
            base
        }
    }
}

/// One service of an agreement as staff set it: the service (a return type or a service), its
/// fees, and the letter sent for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceLine {
    pub service: String,
    pub once_cents: u64,
    pub month_cents: u64,
}

/// The fee lines an agreement sends: a line per service and charge (a service with both a
/// one-time and a monthly fee has two lines).
pub fn fee_lines(services: &[ServiceLine]) -> Vec<FeeLineIn> {
    let mut out = Vec::new();
    for (i, s) in services.iter().enumerate() {
        for (every, cents) in [("once", s.once_cents), ("month", s.month_cents)] {
            if cents == 0 {
                continue;
            }
            out.push(FeeLineIn {
                key: format!("svc-{i}-{every}"),
                kind: "service".into(),
                name: s.service.clone(),
                text: String::new(),
                cents,
                recommended: false,
                services: Vec::new(),
                every: Some(every.into()),
            });
        }
    }
    out
}

/// The agreement's totals: (one-time, monthly).
pub fn totals(services: &[ServiceLine]) -> (u64, u64) {
    services
        .iter()
        .fold((0, 0), |(o, m), s| (o + s.once_cents, m + s.month_cents))
}

/// `{Fee lines}`: one line per service, as "Bookkeeping: $450.00 a month".
pub fn fee_lines_text(services: &[ServiceLine]) -> String {
    services
        .iter()
        .filter(|s| s.once_cents > 0 || s.month_cents > 0)
        .map(|s| {
            let mut parts = Vec::new();
            if s.once_cents > 0 {
                parts.push(money(s.once_cents));
            }
            if s.month_cents > 0 {
                parts.push(format!("{} a month", money(s.month_cents)));
            }
            format!("{}: {}", s.service, parts.join(" and "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Puts a PDF Vibe Files made (the engagement closure letter) in the Portal inbox as
/// `<Client>_<ID>_<name>`, so Intake files it like a document from the portal; written beside
/// it first and moved in whole.
pub fn file_in_inbox(
    root: &std::path::Path,
    client_name: &str,
    client_code: &str,
    file_name: &str,
    pdf: &[u8],
    doc_type: &str,
) -> std::result::Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let name = crate::inbox::inbox_name(client_name, client_code, file_name);
    // QA 2026-11-25: its doc type (the closure template's) noted for Intake before it lands, as
    // the portal's own files are, so Intake files it rather than leaving it to be sorted.
    if let Some(id) = vf_fs::doctypes::load(root)
        .ok()
        .flatten()
        .and_then(|types| vf_fs::doctypes::id_by_name(&types, doc_type))
    {
        let sha: String = Sha256::digest(pdf)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut h = vf_fs::hints::Hint::new(&sha, &name, "portal_closure", "portal");
        h.source = Some("firm".into());
        h.tax_year = file_name.get(..4).and_then(|y| y.parse().ok());
        h.doc_type = Some(id);
        vf_fs::hints::append(root, &h)?;
    }
    let tmp = crate::inbox::tmp_dir(root);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let part = tmp.join(format!("closure-{}.part", crate::inbox::random_tag()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .and_then(|mut f| f.write_all(pdf).and_then(|()| f.sync_all()))
        .and_then(|()| crate::inbox::move_into(&part, &crate::inbox::inbox(root), &name))
        .map_err(|e| {
            let _ = std::fs::remove_file(&part);
            e.to_string()
        })
}

impl Portal {
    /// A client's monthly charges, newest first (live and ended).
    pub fn subscriptions(&self, client_id: &str) -> Result<Vec<Subscription>> {
        #[derive(Deserialize)]
        struct R {
            subscriptions: Vec<Subscription>,
        }
        self.get::<R>("/subscriptions", &[("clientId", client_id)])
            .map(|r| r.subscriptions)
    }

    /// Every live monthly charge (the Deliveries page, every PC's notice of a failed month).
    pub fn live_subscriptions(&self) -> Result<Vec<Subscription>> {
        #[derive(Deserialize)]
        struct R {
            subscriptions: Vec<Subscription>,
        }
        self.get::<R>("/subscriptions", &[])
            .map(|r| r.subscriptions)
    }

    /// A new monthly amount from the next charge (the client is told).
    pub fn change_subscription(&self, id: &str, monthly_cents: u64) -> Result<Subscription> {
        #[derive(Deserialize)]
        struct R {
            subscription: Subscription,
        }
        self.send_json::<R>(
            "PUT",
            &format!("/subscriptions/{id}"),
            &serde_json::json!({ "monthlyCents": monthly_cents }),
        )
        .map(|r| r.subscription)
    }

    /// No more monthly charges (the client is told).
    pub fn stop_subscription(&self, id: &str) -> Result<()> {
        #[derive(Deserialize)]
        struct R {}
        self.delete::<R>(&format!("/subscriptions/{id}"))
            .map(|_| ())
    }

    /// Closing the engagement: the client's open agreements cancelled, the monthly charge
    /// stopped. Answers (agreements cancelled, whether a charge was stopped).
    pub fn end_engagement(&self, client_id: &str) -> Result<(u32, bool)> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R {
            agreements: u32,
            monthly_stopped: bool,
        }
        self.post_empty::<R>(&format!("/clients/{client_id}/end-engagement"))
            .map(|r| (r.agreements, r.monthly_stopped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_totals_and_words() {
        let s = vec![
            ServiceLine {
                service: "1120-S".into(),
                once_cents: 185_000,
                month_cents: 0,
            },
            ServiceLine {
                service: "Bookkeeping".into(),
                once_cents: 25_000,
                month_cents: 45_000,
            },
            ServiceLine {
                service: "Payroll".into(),
                once_cents: 0,
                month_cents: 0,
            },
        ];
        let lines = fee_lines(&s);
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.name.as_str(), l.cents, l.every.as_deref().unwrap()))
                .collect::<Vec<_>>(),
            [
                ("1120-S", 185_000, "once"),
                ("Bookkeeping", 25_000, "once"),
                ("Bookkeeping", 45_000, "month")
            ]
        );
        assert_eq!(totals(&s), (210_000, 45_000));
        assert_eq!(
            fee_lines_text(&s),
            "1120-S: $1,850.00\nBookkeeping: $250.00 and $450.00 a month"
        );
        let mut sub = Subscription {
            id: "s".into(),
            client_id: "c".into(),
            method: "card".into(),
            monthly_cents: 45_000,
            card_fee_cents: 1_350,
            status: "active".into(),
            failed_at: None,
            canceled_at: None,
            delivery_id: "d".into(),
            created_at: String::new(),
        };
        assert_eq!(sub.words(), "$463.50 a month by card");
        sub.failed_at = Some("x".into());
        assert_eq!(
            sub.words(),
            "$463.50 a month by card: the last payment failed"
        );
        sub.canceled_at = Some("y".into());
        assert_eq!(sub.words(), "Monthly charge ended");
    }
}
