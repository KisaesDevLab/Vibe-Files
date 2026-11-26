//! Balance owed (Addendum tax-return delivery §16, owner 2026-11-24; Vibe-Portal D7): staff post
//! a balance from Vibe Files and the client pays it on the portal through the firm's own Stripe
//! account. The amount lives on the portal; Vibe Files keeps a copy of each balance it reads in
//! the client's folder (`<client>/_Vibe/balance.jsonl`, one line per change as read, appended
//! only), for when the portal is out of reach. That copy is never sent to B2.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::{Portal, Result, Status};

/// The states a balance has on the portal.
pub const DUE: &str = "due";
pub const STARTED: &str = "started";
pub const PAID: &str = "paid";
pub const EXPIRED: &str = "expired";
pub const CLEARED: &str = "cleared";
pub const REPLACED: &str = "replaced";

/// The longest memo the portal keeps.
pub const MEMO_MAX: usize = 120;
/// The smallest amount Stripe takes (USD), in cents.
pub const MIN_CENTS: u64 = 50;

/// One event of a balance, in words (as the portal says it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BalanceEvent {
    pub at: String,
    pub kind: String,
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub id: String,
    pub client_id: String,
    pub amount_cents: u64,
    #[serde(default)]
    pub memo: String,
    pub state: String,
    pub expires_at: Option<String>,
    /// Days before a due balance leaves the portal (`None`: no limit). Not kept in the copy:
    /// it changes every day without the balance changing.
    #[serde(default, skip_serializing)]
    pub days_left: Option<u32>,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub remind_days: u32,
    pub reminded_at: Option<String>,
    pub assigned_staff_id: Option<String>,
    #[serde(default)]
    pub posted_by: String,
    pub posted_at: String,
    pub started_at: Option<String>,
    pub paid_at: Option<String>,
    pub method: Option<String>,
    #[serde(default)]
    pub card_fee_cents: u64,
    pub ended_at: Option<String>,
    pub ended_by: Option<String>,
    #[serde(default)]
    pub cleared_pending: bool,
    pub receipt_upload_id: Option<String>,
    /// PA-114: while due, the client's shared files are listed but locked. An older portal
    /// answers nothing: `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_files: Option<bool>,
    /// Only on one client's read (`GET /clients/:id/balance`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<BalanceEvent>,
}

/// "$1,234.56".
pub fn money(cents: u64) -> String {
    let dollars = cents / 100;
    let mut s = dollars.to_string();
    let mut i = s.len() as isize - 3;
    while i > 0 {
        s.insert(i as usize, ',');
        i -= 3;
    }
    format!("${s}.{:02}", cents % 100)
}

/// An amount typed by a person ("240", "$1,240.50", "240.5") in cents; `None` when it is not
/// an amount of dollars and cents.
pub fn parse_money(text: &str) -> Option<u64> {
    let t: String = text
        .trim()
        .trim_start_matches('$')
        .chars()
        .filter(|c| *c != ',' && !c.is_whitespace())
        .collect();
    if t.is_empty() {
        return None;
    }
    let (whole, frac) = match t.split_once('.') {
        Some((w, f)) => (w, f),
        None => (t.as_str(), ""),
    };
    if whole.is_empty() && frac.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
        || frac.len() > 2
        || whole.len() > 9
    {
        return None;
    }
    let w: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let f: u64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    Some(w * 100 + f)
}

/// The day of an RFC 3339 time ("2026-11-24").
fn day(at: &str) -> &str {
    at.get(..10).unwrap_or(at)
}

impl Balance {
    /// Open on the portal: due, or with a payment started.
    pub fn is_open(&self) -> bool {
        self.state == DUE || self.state == STARTED
    }

    /// The client panel's line: "$240.00 on the portal, 9 days left", "Payment started
    /// 2026-11-24", "Paid 2026-11-24", "Expired unpaid 2026-12-08".
    pub fn line(&self) -> String {
        let amount = money(self.amount_cents);
        match self.state.as_str() {
            DUE => {
                let held = if self.hold_files == Some(true) {
                    ", files held"
                } else {
                    ""
                };
                match self.days_left {
                    Some(0) => format!("{amount} on the portal{held}, leaves today"),
                    Some(1) => format!("{amount} on the portal{held}, 1 day left"),
                    Some(n) => format!("{amount} on the portal{held}, {n} days left"),
                    None => format!("{amount} on the portal{held}"),
                }
            }
            STARTED => format!(
                "{amount}: payment started {}",
                day(self.started_at.as_deref().unwrap_or(""))
            ),
            PAID => format!(
                "{amount} paid {}",
                day(self.paid_at.as_deref().unwrap_or(""))
            ),
            EXPIRED => format!(
                "{amount} expired unpaid {}",
                day(self.ended_at.as_deref().unwrap_or(""))
            ),
            CLEARED if self.paid_at.is_some() => format!(
                "{amount} cleared, then paid {}",
                day(self.paid_at.as_deref().unwrap_or(""))
            ),
            CLEARED => format!(
                "{amount} cleared {}",
                day(self.ended_at.as_deref().unwrap_or(""))
            ),
            REPLACED => format!("{amount} replaced by a new amount"),
            other => format!("{amount} ({other})"),
        }
    }

    /// The state in a word, for the Balances list.
    pub fn state_words(&self) -> &'static str {
        match self.state.as_str() {
            DUE => "Due",
            STARTED => "Payment started",
            PAID => "Paid",
            EXPIRED => "Expired unpaid",
            CLEARED if self.paid_at.is_some() => "Cleared, then paid",
            CLEARED => "Cleared",
            REPLACED => "Replaced",
            _ => "Unknown",
        }
    }

    /// The month (`yyyy-mm`) the balance ended in (paid, expired, cleared), for the list's
    /// "this month" filters.
    pub fn ended_month(&self) -> Option<&str> {
        let at = match self.state.as_str() {
            PAID => self.paid_at.as_deref(),
            _ => self.ended_at.as_deref(),
        }?;
        at.get(..7)
    }
}

/// What Post to portal sends.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PostBalance {
    pub amount_cents: u64,
    pub memo: String,
    /// Leave the portal after this many days (`None`: no limit).
    pub expire_days: Option<u32>,
    pub notify: bool,
    pub remind_days: u32,
    pub assigned_staff_id: Option<String>,
    /// PA-114: hold the client's shared files until this is paid (an older portal ignores it
    /// and answers no `holdFiles`).
    pub hold_files: bool,
}

/// PA-114: what needs a newer portal, said once a balance is posted with its files held.
pub const HOLD_NEEDS_PORTAL: &str = "Posted, but the client's files are not held: this needs Vibe Portal 0.5.6 or later. Update the portal first, then Save changes again.";

/// The warning when the hold was asked for and the portal did not take it (an older portal
/// answers no `holdFiles`).
pub fn hold_not_taken(asked: bool, b: &Balance) -> Option<&'static str> {
    (asked && b.hold_files != Some(true)).then_some(HOLD_NEEDS_PORTAL)
}

/// What a post did: the balance, and whether a payment of an earlier amount is pending (the
/// new amount is due once that one settles).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Posted {
    pub balance: Balance,
    #[serde(default)]
    pub pending: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Cleared {
    pub cleared: u32,
    #[serde(default)]
    pub pending: bool,
}

/// A staff member a balance may be assigned to.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BalanceStaff {
    pub id: String,
    pub name: String,
}

impl Status {
    /// The portal has balance cards, set up and turned on (Addendum D7).
    pub fn has_balances(&self) -> bool {
        self.features.iter().any(|f| f == "balances")
    }
}

/// The firm's defaults for a new balance (Settings › Payments on the portal).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub expire_days: u32,
    pub expire_on: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            expire_days: 14,
            expire_on: true,
        }
    }
}

/// `GET /balances`: every client's open balances and those ended in the last 45 days, the
/// firm's defaults, and the portal's staff (for the names of those assigned).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Balances {
    pub balances: Vec<Balance>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub staff: Vec<BalanceStaff>,
}

impl Balances {
    /// The name of a staff member by id ("" when unknown).
    pub fn staff_name(&self, id: Option<&str>) -> &str {
        id.and_then(|id| self.staff.iter().find(|s| s.id == id))
            .map(|s| s.name.as_str())
            .unwrap_or("")
    }
}

impl Portal {
    /// Every client's balances, the defaults and the staff.
    pub fn balances(&self) -> Result<Balances> {
        self.get::<Balances>("/balances", &[])
    }

    /// One client's balances (the last 20), each with what happened to it.
    pub fn client_balances(&self, client_id: &str) -> Result<Vec<Balance>> {
        #[derive(Deserialize)]
        struct R {
            balances: Vec<Balance>,
        }
        self.get::<R>(&format!("/clients/{client_id}/balance"), &[])
            .map(|r| r.balances)
    }

    /// Posts a balance, or changes the one posted.
    pub fn post_balance(&self, client_id: &str, b: &PostBalance) -> Result<Posted> {
        self.send_json(
            "PUT",
            &format!("/clients/{client_id}/balance"),
            &serde_json::to_value(b).expect("serializes"),
        )
    }

    /// Clears what is open (a started payment is still followed).
    pub fn clear_balance(&self, client_id: &str) -> Result<Cleared> {
        self.delete(&format!("/clients/{client_id}/balance"))
    }

    /// Tells the client again; how many messages went.
    pub fn remind_balance(&self, client_id: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct R {
            told: u32,
        }
        self.post_empty::<R>(&format!("/clients/{client_id}/balance/remind"))
            .map(|r| r.told)
    }

    /// Staff a balance of this client may be assigned to.
    pub fn balance_staff(&self, client_id: &str) -> Result<Vec<BalanceStaff>> {
        #[derive(Deserialize)]
        struct R {
            staff: Vec<BalanceStaff>,
        }
        self.get::<R>("/balance-staff", &[("clientId", client_id)])
            .map(|r| r.staff)
    }
}

// ---- The copy in the client's folder ----------------------------------------------------------

pub const COPY_FILE: &str = "balance.jsonl";

/// `<cabinet>/<client folder>/_Vibe/balance.jsonl`.
pub fn copy_path(root: &Path, folder_rel: &str) -> PathBuf {
    root.join(folder_rel)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join(COPY_FILE)
}

/// One line of the copy: a balance as read, and when and by whom.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyLine {
    pub v: u32,
    pub read_at: String,
    pub read_by: String,
    pub balance: Balance,
}

/// The newest line of each balance in a copy's text.
pub fn latest(text: &str) -> Vec<CopyLine> {
    let mut out: Vec<CopyLine> = Vec::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(c) = serde_json::from_str::<CopyLine>(l) else {
            continue;
        };
        match out.iter_mut().find(|o| o.balance.id == c.balance.id) {
            Some(o) => *o = c,
            None => out.push(c),
        }
    }
    out
}

/// The copy's balances, newest line of each (for when the portal cannot be reached), newest
/// posted first.
pub fn read_copy(root: &Path, folder_rel: &str) -> Vec<CopyLine> {
    let text = std::fs::read_to_string(copy_path(root, folder_rel)).unwrap_or_default();
    let mut out = latest(&text);
    out.sort_by(|a, b| b.balance.posted_at.cmp(&a.balance.posted_at));
    out
}

/// The balance as the copy keeps it: what changes it, without the events (they are on the
/// portal and in the receipt) and without the days left (they change daily).
fn kept(b: &Balance) -> Balance {
    Balance {
        events: Vec::new(),
        days_left: None,
        ..b.clone()
    }
}

/// The line to add to the copy for `b`, or `None` when the copy already ends with it as it is.
/// `copy` is the copy's newest lines ([`latest`]).
pub fn copy_line(copy: &[CopyLine], b: &Balance, read_at: &str, read_by: &str) -> Option<String> {
    let b = kept(b);
    if copy.iter().any(|c| c.balance == b) {
        return None;
    }
    Some(
        serde_json::to_string(&CopyLine {
            v: 1,
            read_at: read_at.into(),
            read_by: read_by.into(),
            balance: b,
        })
        .expect("serializes"),
    )
}

/// What changed for one balance since the copy's last line of it: `paid` and `expired` are
/// the changes the firm is told of (audit rows, a notice); `None` for anything else.
pub fn change_of(copy: &[CopyLine], b: &Balance) -> Option<&'static str> {
    let before = copy
        .iter()
        .find(|c| c.balance.id == b.id)
        .map(|c| (c.balance.state.as_str(), c.balance.paid_at.is_some()));
    let paid_now = b.paid_at.is_some();
    match before {
        Some((_, true)) => None,
        _ if paid_now => Some(PAID),
        Some((s, _)) if s == EXPIRED => None,
        _ if b.state == EXPIRED => Some(EXPIRED),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A balance as the portal's tests answer it (Vibe-Portal balances.test.ts).
    pub(crate) const BALANCE_JSON: &str = r#"{
      "id":"b1111111-1111-4111-8111-111111111111","clientId":"11111111-1111-4111-8111-111111111111",
      "amountCents":24000,"memo":"2025 bookkeeping, Oct–Dec","state":"due",
      "expiresAt":"2026-12-08T12:00:00.000Z","daysLeft":9,"notify":true,"remindDays":7,
      "remindedAt":null,"assignedStaffId":null,"postedBy":"PC1\\kurt",
      "postedAt":"2026-11-24T12:00:00.000Z","startedAt":null,"paidAt":null,"method":null,
      "cardFeeCents":0,"endedAt":null,"endedBy":null,"clearedPending":false,"receiptUploadId":null}"#;

    #[test]
    fn money_both_ways() {
        assert_eq!(money(24_000), "$240.00");
        assert_eq!(money(123_456_789), "$1,234,567.89");
        assert_eq!(money(5), "$0.05");
        assert_eq!(parse_money("$1,240.5"), Some(124_050));
        assert_eq!(parse_money(" 240 "), Some(24_000));
        assert_eq!(parse_money(".75"), Some(75));
        assert_eq!(parse_money("12.345"), None);
        assert_eq!(parse_money("abc"), None);
        assert_eq!(parse_money(""), None);
        assert_eq!(parse_money("."), None);
    }

    #[test]
    fn reads_the_portal_balance_and_says_it() {
        let b: Balance = serde_json::from_str(BALANCE_JSON).unwrap();
        assert_eq!(b.line(), "$240.00 on the portal, 9 days left");
        assert!(b.is_open());
        let started = Balance {
            state: STARTED.into(),
            started_at: Some("2026-11-25T10:00:00Z".into()),
            ..b.clone()
        };
        assert_eq!(started.line(), "$240.00: payment started 2026-11-25");
        let paid = Balance {
            state: PAID.into(),
            paid_at: Some("2026-11-26T10:00:00Z".into()),
            ..b
        };
        assert_eq!(paid.ended_month(), Some("2026-11"));
        assert_eq!(paid.state_words(), "Paid");
    }

    #[test]
    fn the_copy_gets_a_line_only_on_a_change_and_says_paid_and_expired_once() {
        let t = tempfile::tempdir().unwrap();
        let b: Balance = serde_json::from_str(BALANCE_JSON).unwrap();
        let first = copy_line(&[], &b, "2026-11-24T13:00:00Z", "PC1\\kurt").unwrap();
        let copy = latest(&first);
        // The days left change daily: not a change of the balance.
        let tomorrow = Balance {
            days_left: Some(8),
            ..b.clone()
        };
        assert_eq!(copy_line(&copy, &tomorrow, "later", "PC2\\ann"), None);
        assert_eq!(change_of(&copy, &b), None);
        let expired = Balance {
            state: EXPIRED.into(),
            ended_at: Some("2026-12-08T12:10:00Z".into()),
            ..b.clone()
        };
        assert_eq!(change_of(&copy, &expired), Some(EXPIRED));
        let second = copy_line(&copy, &expired, "later", "PC2\\ann").unwrap();
        let p = copy_path(t.path(), "Smith, John (1001)");
        assert!(p.ends_with("Smith, John (1001)/_Vibe/balance.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("{first}\n{second}\n")).unwrap();
        let back = read_copy(t.path(), "Smith, John (1001)");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].balance.state, EXPIRED);
        assert_eq!(change_of(&back, &expired), None, "told once");
        // A bank payment that clears after it was cleared by staff: paid, once.
        let paid = Balance {
            state: CLEARED.into(),
            paid_at: Some("2026-12-09T09:00:00Z".into()),
            ..expired
        };
        assert_eq!(change_of(&back, &paid), Some(PAID));
        assert!(!first.contains("daysLeft"));
    }

    #[test]
    fn a_hold_an_older_portal_did_not_take_is_said() {
        let mut b: Balance = serde_json::from_value(serde_json::json!({
            "id": "b1", "clientId": "c1", "amountCents": 125000, "state": "due",
            "expiresAt": null, "remindedAt": null, "assignedStaffId": null,
            "postedAt": "2026-11-26T10:00:00Z", "startedAt": null, "paidAt": null,
            "method": null, "endedAt": null, "endedBy": null, "receiptUploadId": null
        }))
        .unwrap();
        assert_eq!(b.hold_files, None, "an older portal says nothing");
        assert_eq!(hold_not_taken(true, &b), Some(HOLD_NEEDS_PORTAL));
        assert_eq!(hold_not_taken(false, &b), None);
        assert_eq!(b.line(), "$1,250.00 on the portal");
        b.hold_files = Some(true);
        assert_eq!(hold_not_taken(true, &b), None);
        assert_eq!(b.line(), "$1,250.00 on the portal, files held");
        let p = PostBalance {
            amount_cents: 125000,
            memo: String::new(),
            expire_days: None,
            notify: true,
            remind_days: 0,
            assigned_staff_id: None,
            hold_files: true,
        };
        assert_eq!(serde_json::to_value(&p).unwrap()["holdFiles"], true);
    }

    #[test]
    fn a_post_sends_null_for_no_limit() {
        let p = PostBalance {
            amount_cents: 24_000,
            memo: "x".into(),
            expire_days: None,
            notify: true,
            remind_days: 0,
            assigned_staff_id: None,
            hold_files: false,
        };
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({ "amountCents": 24000, "memo": "x", "expireDays": null,
                "notify": true, "remindDays": 0, "assignedStaffId": null,
                "holdFiles": false })
        );
    }
}
