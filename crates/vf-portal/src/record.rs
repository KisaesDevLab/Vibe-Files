//! Checking who asks for portal access against the firm's client record (Vibe Portal
//! Addendum 01, layer B).
//!
//! The client record is only here: the client list's columns. The firm says in Admin › Portal
//! which column holds the client's email, mobile number and date of birth; a request is compared
//! with the chosen client's row on this PC, and only what matched is sent to the portal.
//!
//! A request matches (Vibe Portal P84) when the contact the person confirmed with a code is the
//! record's email or mobile, or the date of birth in their ID's barcode is the record's and the
//! portal did not flag the ID's name as different from the request's.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::api::AccessRequest;

/// The portal's flag for an ID whose name is not the request's.
pub const FLAG_NAME_DIFFERS: &str = "name_differs";

/// Which client-list columns hold what a request is compared with, by token name (the key of
/// `Client.extra`). `None`: the firm keeps no such column, and nothing is compared with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordColumns {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub mobile: Option<String>,
    #[serde(default)]
    pub dob: Option<String>,
}

impl RecordColumns {
    pub fn any(&self) -> bool {
        self.email.is_some() || self.mobile.is_some() || self.dob.is_some()
    }
}

/// How one thing on the request compares with the client's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Matches,
    Differs,
    /// No column was chosen for it, or this client's row is empty there.
    NotInRecord,
    /// The request has nothing to compare: confirmed by the other contact, or no date of birth
    /// was read from an ID.
    NotOnRequest,
}

impl Line {
    pub fn words(self) -> &'static str {
        match self {
            Self::Matches => "matches the record",
            Self::Differs => "differs from the record",
            Self::NotInRecord => "not in the record",
            Self::NotOnRequest => "not on the request",
        }
    }
}

/// A request beside the row of the client staff chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparison {
    pub email: Line,
    pub mobile: Line,
    pub dob: Line,
    /// The portal flagged the name on the ID as different from the name on the request.
    pub name_differs: bool,
}

impl Comparison {
    /// What matched, in the portal's words (`email`, `mobile`, `dob`): empty when the request
    /// does not match the record. A date of birth counts only with the name flag clear.
    pub fn matched(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.email == Line::Matches {
            out.push("email");
        }
        if self.mobile == Line::Matches {
            out.push("mobile");
        }
        if self.dob == Line::Matches && !self.name_differs {
            out.push("dob");
        }
        out
    }
}

/// An email address as compared: trimmed, lower case.
fn email_key(s: &str) -> String {
    s.trim().to_lowercase()
}

/// A phone number as compared: its last ten digits (the country code and punctuation aside), or
/// `None` when it has fewer than seven. An extension ("x12", "ext. 12", "#12") is left off.
fn phone_key(s: &str) -> Option<String> {
    let lower = s.to_lowercase();
    let mut digits = String::new();
    let mut rest = lower.as_str();
    while let Some(c) = rest.chars().next() {
        if digits.len() >= 7 && (c == 'x' || c == '#' || rest.starts_with("ext")) {
            break;
        }
        if c.is_ascii_digit() {
            digits.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    if digits.len() < 7 {
        return None;
    }
    Some(digits[digits.len().saturating_sub(10)..].to_string())
}

/// The phone numbers of a cell: one, or several set apart by `;`, `,`, `/`, `|` or "or".
fn phones(cell: &str) -> impl Iterator<Item = String> + '_ {
    cell.split([';', ',', '/', '|'])
        .flat_map(|part| part.split(" or "))
        .filter_map(phone_key)
}

/// The email addresses of a cell: one, or several, written plainly, as `Name <a@b>` or as
/// `mailto:a@b`.
fn emails(cell: &str) -> impl Iterator<Item = String> + '_ {
    cell.split([';', ',', ' ', '<', '>', '(', ')'])
        .map(|t| {
            let t = t.trim();
            let lower = t.to_lowercase();
            lower.strip_prefix("mailto:").unwrap_or(&lower).to_string()
        })
        .filter(|t| t.contains('@'))
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// The month a word names ("Jan", "January", "Sept."), 1 to 12.
fn month_named(word: &str) -> Option<u32> {
    let w = word.trim_end_matches('.').to_lowercase();
    if w.len() < 3 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| w.starts_with(m))
        .map(|i| i as u32 + 1)
}

/// A date as written in a client list, as (year, month, day):
/// - `1990-01-06`, `1990/01/06`, `19900106`;
/// - `1/6/1990`, `01-06-1990`, `1.6.1990`: month first, as US lists write it, unless the first
///   number is over 12 (`13/1/1990` is 13 January);
/// - `Jan 6 1990`, `January 6, 1990`, `6 Jan 1990`, `6-Jan-1990`;
/// - a spreadsheet's day number (`32879` is 1990-01-06).
///
/// A two-digit year is not guessed at. A day-first list whose days are 12 or less reads month
/// first: such a row shows as "differs", never as a match it is not.
pub fn parse_date(s: &str) -> Option<(u32, u32, u32)> {
    let ok = |y: u32, m: u32, d: u32| {
        ((1900..=2199).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d))
            .then_some((y, m, d))
    };
    let whole = s.trim();
    // A month by name.
    let words: Vec<&str> = whole
        .split([' ', ',', '-', '/', '.'])
        .filter(|w| !w.is_empty())
        .collect();
    if let Some(at) = words.iter().position(|w| month_named(w).is_some()) {
        let m = month_named(words[at])?;
        let nums: Vec<u32> = words
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != at)
            .filter_map(|(_, w)| w.parse().ok())
            .collect();
        return match nums.as_slice() {
            [a, b, ..] if *a > 31 => ok(*a, m, *b),
            [a, b, ..] => ok(*b, m, *a),
            _ => None,
        };
    }
    // A time after the date (a spreadsheet's "1/6/1990 0:00") is dropped.
    let t = whole.split([' ', 'T']).next().unwrap_or("");
    let parts: Vec<&str> = t.split(['-', '/', '.']).collect();
    let num = |p: &str| -> Option<u32> {
        (!p.is_empty() && p.len() <= 4 && p.chars().all(|c| c.is_ascii_digit()))
            .then(|| p.parse().ok())
            .flatten()
    };
    match parts.as_slice() {
        [one] if one.len() == 8 && one.chars().all(|c| c.is_ascii_digit()) => ok(
            one[0..4].parse().ok()?,
            one[4..6].parse().ok()?,
            one[6..8].parse().ok()?,
        ),
        // A spreadsheet's day number: days since 30 December 1899.
        [one] if (4..=6).contains(&one.len()) && one.chars().all(|c| c.is_ascii_digit()) => {
            use chrono::Datelike;
            let day = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?
                .checked_add_days(chrono::Days::new(one.parse().ok()?))?;
            ok(u32::try_from(day.year()).ok()?, day.month(), day.day())
        }
        [a, b, c] if a.len() == 4 => ok(num(a)?, num(b)?, num(c)?),
        [a, b, c] if c.len() == 4 => {
            let (first, second) = (num(a)?, num(b)?);
            // Over 12 cannot be a month: the list writes the day first.
            if first > 12 {
                ok(num(c)?, second, first)
            } else {
                ok(num(c)?, first, second)
            }
        }
        _ => None,
    }
}

/// Each client's row of the client list, by its ID. An ID is looked up as written; when no
/// client has it so, without regard to case, but only if one client alone answers to it (two
/// IDs that differ only by case are two clients: neither stands in for the other).
#[derive(Debug, Clone, Default)]
pub struct Rows {
    exact: BTreeMap<String, BTreeMap<String, String>>,
    /// Lower-case ID to the one ID written so, or `None` when several are.
    folded: BTreeMap<String, Option<String>>,
}

impl Rows {
    pub fn clear(&mut self) {
        self.exact.clear();
        self.folded.clear();
    }

    pub fn insert(&mut self, id: &str, extra: BTreeMap<String, String>) {
        let id = id.trim().to_string();
        self.folded
            .entry(id.to_lowercase())
            .and_modify(|e| {
                if e.as_deref() != Some(id.as_str()) {
                    *e = None;
                }
            })
            .or_insert_with(|| Some(id.clone()));
        self.exact.insert(id, extra);
    }

    pub fn get(&self, id: &str) -> Option<&BTreeMap<String, String>> {
        let id = id.trim();
        self.exact.get(id).or_else(|| {
            self.folded
                .get(&id.to_lowercase())
                .and_then(|one| one.as_deref())
                .and_then(|k| self.exact.get(k))
        })
    }
}

/// The row's value in a chosen column, when the column was chosen and the row has something.
fn cell<'a>(extra: &'a BTreeMap<String, String>, column: &Option<String>) -> Option<&'a str> {
    let v = extra.get(column.as_deref()?)?.trim();
    (!v.is_empty()).then_some(v)
}

/// Compares `request` with a client's row (`extra`: the client list's columns by token name).
/// Only the contact the person confirmed with a code is compared: the other one on the request
/// is only what they typed.
pub fn compare(
    request: &AccessRequest,
    extra: &BTreeMap<String, String>,
    columns: &RecordColumns,
) -> Comparison {
    let confirmed = request.confirmed_contact.as_deref().unwrap_or("").trim();
    let by_email = confirmed.contains('@');
    let email = if !by_email {
        Line::NotOnRequest
    } else {
        match cell(extra, &columns.email) {
            None => Line::NotInRecord,
            // A cell may hold more than one address.
            Some(v) => {
                if emails(v).any(|a| a == email_key(confirmed)) {
                    Line::Matches
                } else {
                    Line::Differs
                }
            }
        }
    };
    let mobile = match (by_email || confirmed.is_empty(), phone_key(confirmed)) {
        (true, _) | (_, None) => Line::NotOnRequest,
        (false, Some(asked)) => match cell(extra, &columns.mobile) {
            None => Line::NotInRecord,
            Some(v) => {
                if phones(v).any(|n| n == asked) {
                    Line::Matches
                } else {
                    Line::Differs
                }
            }
        },
    };
    let scanned = request
        .id_scan
        .as_ref()
        .and_then(|s| s.dob.as_deref())
        .and_then(parse_date);
    let dob = match scanned {
        None => Line::NotOnRequest,
        Some(on_id) => match cell(extra, &columns.dob) {
            None => Line::NotInRecord,
            Some(v) => match parse_date(v) {
                Some(d) if d == on_id => Line::Matches,
                // A date that cannot be read is not the ID's date.
                _ => Line::Differs,
            },
        },
    };
    Comparison {
        email,
        mobile,
        dob,
        name_differs: request.id_flags.iter().any(|f| f == FLAG_NAME_DIFFERS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::IdScan;

    fn request(confirmed: &str, dob: Option<&str>, flags: &[&str]) -> AccessRequest {
        AccessRequest {
            id: "r1".into(),
            name: "Alexander Sample".into(),
            email: None,
            phone: None,
            confirmed_contact: Some(confirmed.into()),
            client_as: "Sample".into(),
            note: String::new(),
            created_at: "2026-11-14T10:00:00Z".into(),
            id_scan: dob.map(|d| IdScan {
                dob: Some(d.into()),
                ..IdScan::default()
            }),
            id_flags: flags.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    fn row(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn columns() -> RecordColumns {
        RecordColumns {
            email: Some("Email".into()),
            mobile: Some("Cell".into()),
            dob: Some("DOB".into()),
        }
    }

    #[test]
    fn dates_are_read_as_client_lists_write_them() {
        for s in [
            "1990-01-06",
            "1990/1/6",
            "19900106",
            "1/6/1990",
            "01-06-1990",
            "1.6.1990",
            " 1/6/1990 0:00 ",
            "1990-01-06T00:00:00",
        ] {
            assert_eq!(parse_date(s), Some((1990, 1, 6)), "{s}");
        }
        for s in [
            "Jan 6 1990",
            "January 6, 1990",
            "6 Jan 1990",
            "6-Jan-1990",
            "Jan. 6, 1990",
            "1990 January 6",
            "32879",
        ] {
            assert_eq!(parse_date(s), Some((1990, 1, 6)), "{s}");
        }
        // Over 12 cannot be a month: day first.
        assert_eq!(parse_date("13/1/1990"), Some((1990, 1, 13)));
        assert_eq!(parse_date("31.12.1990"), Some((1990, 12, 31)));
        for s in [
            "",
            "1/6/90",
            "Janus 6",
            "13/40/1990",
            "1990-13-01",
            "0000-01-01",
            "12",
        ] {
            assert_eq!(parse_date(s), None, "{s}");
        }
    }

    #[test]
    fn the_confirmed_email_matches_whatever_its_case_or_company() {
        let extra = row(&[
            ("Email", "Old@Example.test; ALEX@example.test"),
            ("Cell", ""),
        ]);
        let c = compare(
            &request(" alex@Example.test ", None, &[]),
            &extra,
            &columns(),
        );
        assert_eq!(
            (c.email, c.mobile, c.dob),
            (Line::Matches, Line::NotOnRequest, Line::NotOnRequest)
        );
        assert_eq!(c.matched(), vec!["email"]);
        let other = compare(&request("mary@example.test", None, &[]), &extra, &columns());
        assert_eq!(other.email, Line::Differs);
        assert!(other.matched().is_empty());
    }

    #[test]
    fn the_confirmed_mobile_matches_by_its_last_ten_digits() {
        let extra = row(&[("Cell", "(555) 010-1234 / 555.010.9999")]);
        let c = compare(&request("+15550101234", None, &[]), &extra, &columns());
        assert_eq!((c.email, c.mobile), (Line::NotOnRequest, Line::Matches));
        assert_eq!(c.matched(), vec!["mobile"]);
        let c = compare(&request("+15550100000", None, &[]), &extra, &columns());
        assert_eq!(c.mobile, Line::Differs);
        // The row has no email: said, not counted as a difference.
        let c = compare(&request("alex@example.test", None, &[]), &extra, &columns());
        assert_eq!(c.email, Line::NotInRecord);
    }

    #[test]
    fn a_date_of_birth_counts_only_with_the_name_clear() {
        let extra = row(&[("DOB", "1/6/1990"), ("Email", "someone@else.test")]);
        let c = compare(
            &request("alex@example.test", Some("1990-01-06"), &[]),
            &extra,
            &columns(),
        );
        assert_eq!((c.email, c.dob), (Line::Differs, Line::Matches));
        assert_eq!(c.matched(), vec!["dob"]);
        let flagged = compare(
            &request(
                "alex@example.test",
                Some("1990-01-06"),
                &["expired", "name_differs"],
            ),
            &extra,
            &columns(),
        );
        assert_eq!(flagged.dob, Line::Matches);
        assert!(flagged.matched().is_empty());
        let wrong = compare(
            &request("alex@example.test", Some("1990-01-07"), &[]),
            &extra,
            &columns(),
        );
        assert_eq!(wrong.dob, Line::Differs);
        // A row whose date cannot be read does not match.
        let odd = compare(
            &request("alex@example.test", Some("1990-01-06"), &[]),
            &row(&[("DOB", "sometime in 1990")]),
            &columns(),
        );
        assert_eq!(odd.dob, Line::Differs);
    }

    #[test]
    fn extensions_names_and_several_in_a_cell_are_read() {
        let cols = columns();
        for cell in [
            "555-010-1234 x12",
            "(555) 010-1234 ext. 7",
            "555.010.9999 or 555.010.1234",
            "1 555 010 1234 #3 | 555 010 0000",
        ] {
            let c = compare(
                &request("+15550101234", None, &[]),
                &row(&[("Cell", cell)]),
                &cols,
            );
            assert_eq!(c.mobile, Line::Matches, "{cell}");
        }
        for cell in [
            "Alex Sample <ALEX@example.test>",
            "mailto:alex@example.test",
            "old@example.test, Alex (alex@example.test)",
        ] {
            let c = compare(
                &request("alex@example.test", None, &[]),
                &row(&[("Email", cell)]),
                &cols,
            );
            assert_eq!(c.email, Line::Matches, "{cell}");
        }
        let c = compare(
            &request("alex@example.test", None, &[]),
            &row(&[("Email", "Alex Sample")]),
            &cols,
        );
        assert_eq!(c.email, Line::Differs);
    }

    #[test]
    fn a_row_is_found_by_its_id_and_never_by_another_clients() {
        let mut rows = Rows::default();
        rows.insert(" AB-1 ", row(&[("Email", "one@example.test")]));
        rows.insert("cd-2", row(&[("Email", "two@example.test")]));
        // As written, trimmed; and whatever the case, when one client alone answers to it.
        assert_eq!(rows.get("AB-1").unwrap()["Email"], "one@example.test");
        assert_eq!(rows.get("ab-1 ").unwrap()["Email"], "one@example.test");
        assert!(rows.get("zz-9").is_none());
        // Two IDs that differ only by case are two clients.
        rows.insert("CD-2", row(&[("Email", "three@example.test")]));
        assert_eq!(rows.get("cd-2").unwrap()["Email"], "two@example.test");
        assert_eq!(rows.get("CD-2").unwrap()["Email"], "three@example.test");
        assert!(rows.get("Cd-2").is_none());
        rows.clear();
        assert!(rows.get("AB-1").is_none());
    }

    #[test]
    fn with_no_columns_chosen_nothing_matches() {
        let extra = row(&[("Email", "alex@example.test"), ("DOB", "1/6/1990")]);
        let none = RecordColumns::default();
        assert!(!none.any());
        let c = compare(
            &request("alex@example.test", Some("1990-01-06"), &[]),
            &extra,
            &none,
        );
        assert_eq!((c.email, c.dob), (Line::NotInRecord, Line::NotInRecord));
        assert!(c.matched().is_empty());
    }
}
