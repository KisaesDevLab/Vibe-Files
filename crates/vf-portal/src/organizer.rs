//! Organizer links (PA-113, portal P117, owner 2026-11-25): the firm's online organizer app
//! (Stanford Tax) exports each client's questionnaire link; Vibe Files reads the export, matches
//! each row to a client, and sends the links to the portal, which shows an Organizer card until
//! the tax return delivery for that year is created or staff turn it off.
//!
//! Matching (owner 2026-11-25, "what about using name and email match to locate?"), surest first:
//! - the export's UltraTax ID is a client's ID: matched, unless the row's email is another
//!   client's or the names have nothing in common, which asks;
//! - an email of the row is a Portal contact of one client whose name agrees: matched; of one
//!   whose name does not, or of several: asks;
//! - the name alone (the same, or the same words): a suggestion, not ticked (owner Q&A);
//! - nothing: staff pick the client.
//!
//! A wrong match would put one family's organizer on another's portal, so nothing is sent
//! without the preview, and only https links on the organizer's host are taken.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use vf_fs::contacts::{ClientContacts, normal_email};
use vf_fs::namematch::{self, Name, Tier};

use crate::api::{Portal, Result};

/// The organizer apps whose links are taken (the portal checks the same list).
pub const HOSTS: [&str; 1] = ["clients.stanfordtax.com"];

/// Whether a link is one the portal will show: https on an organizer host, nothing else.
pub fn allowed_link(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    HOSTS.iter().any(|h| host.eq_ignore_ascii_case(h))
}

/// What the export says of the client's questionnaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    NotSeen,
    InProgress,
    Submitted,
    #[default]
    Unknown,
}

impl Status {
    fn of(s: &str) -> Status {
        match s.trim().to_lowercase().as_str() {
            "not seen yet" | "not seen" | "not started" => Status::NotSeen,
            "in progress" | "started" => Status::InProgress,
            "submitted" | "complete" | "completed" => Status::Submitted,
            _ => Status::Unknown,
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Status::NotSeen => "Not seen yet",
            Status::InProgress => "In progress",
            Status::Submitted => "Submitted",
            Status::Unknown => "Status not known",
        }
    }
}

/// One row of the export.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportRow {
    /// Its line in the file (the header is line 1).
    pub line: usize,
    pub client: String,
    /// Normalised.
    pub emails: Vec<String>,
    pub tax_year: Option<i32>,
    pub return_type: String,
    pub status: Status,
    pub url: String,
    /// The tax software's client ID, when the export names its source.
    pub source_id: String,
    /// The export says the return was delivered through the organizer app.
    pub delivered: bool,
}

/// Reads the organizer app's export (CSV, its own column names).
pub fn read_export(path: &Path) -> std::result::Result<Vec<ExportRow>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
    parse_export(&bytes)
}

pub fn parse_export(bytes: &[u8]) -> std::result::Result<Vec<ExportRow>, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut r = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(bytes);
    let headers: Vec<String> = r
        .headers()
        .map_err(|e| format!("Not a CSV file: {e}"))?
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let col = |names: &[&str]| headers.iter().position(|h| names.contains(&h.as_str()));
    let (Some(client), Some(link)) = (
        col(&["client", "client name", "name"]),
        col(&["questionnaire link", "organizer link", "link"]),
    ) else {
        return Err(
            "This is not an organizer export: it needs a Client column and a Questionnaire link column."
                .into(),
        );
    };
    let emails = col(&["emails", "email"]);
    let year = col(&["tax year", "year"]);
    let kind = col(&["return type"]);
    let status = col(&["questionnaire status", "status"]);
    let source_id = col(&["source id"]);
    let delivered = col(&["tax return delivery status"]);
    let mut out = Vec::new();
    for (i, rec) in r.records().enumerate() {
        let rec = rec.map_err(|e| format!("Line {}: {e}", i + 2))?;
        let get = |c: Option<usize>| c.and_then(|c| rec.get(c)).unwrap_or("").trim().to_string();
        let row = ExportRow {
            line: i + 2,
            client: get(Some(client)),
            emails: get(emails)
                .split([',', ';', ' '])
                .map(|e| e.trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\'' | '(' | ')')))
                .filter_map(normal_email)
                .collect(),
            tax_year: get(year).parse().ok(),
            return_type: get(kind),
            status: Status::of(&get(status)),
            url: get(Some(link)),
            source_id: get(source_id),
            delivered: {
                let d = get(delivered).to_lowercase();
                d.starts_with("delivered") || d == "complete" || d == "completed"
            },
        };
        if !row.client.is_empty() || !row.url.is_empty() {
            out.push(row);
        }
    }
    Ok(out)
}

/// How a row found its client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// The tax software's client ID.
    Id,
    /// An email on file for the client, and its name agrees.
    EmailAndName,
    /// The name alone: a suggestion.
    Name,
    /// Something disagrees: staff choose.
    Check,
    /// No client found.
    None,
}

/// A client a row may be for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub client_id: String,
    pub code: String,
    pub name: String,
}

impl Candidate {
    fn of(c: &ClientContacts) -> Candidate {
        Candidate {
            client_id: c.client_id.clone(),
            code: c.code.clone(),
            name: c.name.clone(),
        }
    }

    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.code)
    }
}

/// A row of the export and the client it would go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matched {
    pub row: ExportRow,
    pub by: By,
    /// The client chosen (the surest candidate), if any.
    pub client: Option<Candidate>,
    /// Others it may be for (Check), the chosen one first.
    pub candidates: Vec<Candidate>,
    /// Sent unless unticked; a name-only match or a check starts unticked.
    pub ticked: bool,
    /// What staff should know: why it asks, or why it cannot be sent.
    pub note: String,
    /// Not sendable: a link the portal won't take, or another tax year.
    pub refused: bool,
}

fn same_code(a: &str, b: &str) -> bool {
    let n = |s: &str| s.trim().trim_start_matches('0').to_lowercase();
    // QA 2026-11-25: an ID of "0" (or none) on either side matches nothing.
    !n(a).is_empty() && n(a) == n(b)
}

/// A close name counts as agreeing only from this score (QA 2026-11-25: any score did, so one
/// shared word, "John Smith" and "Smith Plumbing LLC", ticked the row for the business).
const NEAR_AGREES: f64 = 0.8;

fn names_agree(a: &str, b: &str) -> Option<Tier> {
    match namematch::compare(&Name::of(a, None), &Name::of(b, None)) {
        Some((_, t @ (Tier::Exact | Tier::Rule))) => Some(t),
        Some((score, Tier::Near)) if score >= NEAR_AGREES => Some(Tier::Near),
        _ => None,
    }
}

/// Matches each row to a client of `dir` for `tax_year`.
pub fn match_rows(rows: &[ExportRow], dir: &[ClientContacts], tax_year: i32) -> Vec<Matched> {
    // Email → the clients it is a Portal contact of.
    let mut by_email: BTreeMap<&str, Vec<&ClientContacts>> = BTreeMap::new();
    for c in dir {
        for p in &c.planned.people {
            if let Some(e) = p.email.as_deref() {
                let list = by_email.entry(e).or_default();
                if !list.iter().any(|x| x.client_id == c.client_id) {
                    list.push(c);
                }
            }
        }
    }
    rows.iter()
        .map(|row| {
            let mut m = match_one(row, dir, &by_email);
            if !allowed_link(&row.url) {
                m.refused = true;
                m.ticked = false;
                m.note = format!("Not an organizer link ({} only)", HOSTS.join(", "));
            } else if row.tax_year.is_some_and(|y| y != tax_year) {
                m.refused = true;
                m.ticked = false;
                m.note = format!("For {}, not {tax_year}", row.tax_year.unwrap_or_default());
            }
            m
        })
        .collect()
}

fn match_one(
    row: &ExportRow,
    dir: &[ClientContacts],
    by_email: &BTreeMap<&str, Vec<&ClientContacts>>,
) -> Matched {
    let base = Matched {
        row: row.clone(),
        by: By::None,
        client: None,
        candidates: Vec::new(),
        ticked: false,
        note: String::new(),
        refused: false,
    };
    // The clients the row's emails are on, in order, once each.
    let mut on_email: Vec<&ClientContacts> = Vec::new();
    for e in &row.emails {
        for c in by_email.get(e.as_str()).into_iter().flatten() {
            if !on_email.iter().any(|x| x.client_id == c.client_id) {
                on_email.push(c);
            }
        }
    }
    let agrees = |c: &ClientContacts| names_agree(&row.client, &c.name).is_some();

    // 1. The tax software's ID.
    if let Some(c) = dir.iter().find(|c| same_code(&row.source_id, &c.code)) {
        let elsewhere: Vec<&&ClientContacts> = on_email
            .iter()
            .filter(|o| o.client_id != c.client_id)
            .collect();
        let email_here = on_email.iter().any(|o| o.client_id == c.client_id);
        if !elsewhere.is_empty() && !email_here {
            let mut candidates = vec![Candidate::of(c)];
            candidates.extend(elsewhere.iter().map(|o| Candidate::of(o)));
            return Matched {
                by: By::Check,
                client: Some(Candidate::of(c)),
                note: format!(
                    "ID {} is {}; the email is {}'s",
                    row.source_id,
                    c.name,
                    elsewhere
                        .iter()
                        .map(|o| format!("{} ({})", o.name, o.code))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                candidates,
                ..base
            };
        }
        if !agrees(c) && !email_here {
            return Matched {
                by: By::Check,
                client: Some(Candidate::of(c)),
                candidates: vec![Candidate::of(c)],
                note: format!("ID {} is {}: a different name", row.source_id, c.name),
                ..base
            };
        }
        return Matched {
            by: By::Id,
            client: Some(Candidate::of(c)),
            candidates: vec![Candidate::of(c)],
            ticked: true,
            ..base
        };
    }

    // 2. An email on file.
    if !on_email.is_empty() {
        let agreeing: Vec<&&ClientContacts> = on_email.iter().filter(|c| agrees(c)).collect();
        if on_email.len() == 1 && agreeing.len() == 1 {
            let c = on_email[0];
            return Matched {
                by: By::EmailAndName,
                client: Some(Candidate::of(c)),
                candidates: vec![Candidate::of(c)],
                ticked: true,
                ..base
            };
        }
        let first = agreeing.first().map(|c| **c).unwrap_or(on_email[0]);
        let mut candidates = vec![Candidate::of(first)];
        candidates.extend(
            on_email
                .iter()
                .filter(|c| c.client_id != first.client_id)
                .map(|c| Candidate::of(c)),
        );
        return Matched {
            by: By::Check,
            client: Some(Candidate::of(first)),
            note: if on_email.len() > 1 {
                format!("The email is on {} clients", on_email.len())
            } else {
                format!("The email is {}'s, a different name", first.name)
            },
            candidates,
            ..base
        };
    }

    // 3. The name alone: the same, or the same words (not "close").
    let mut named: Vec<(Tier, &ClientContacts)> = dir
        .iter()
        .filter_map(|c| match names_agree(&row.client, &c.name) {
            Some(t @ (Tier::Exact | Tier::Rule)) => Some((t, c)),
            _ => None,
        })
        .collect();
    named.sort_by_key(|(t, _)| *t);
    if let Some((_, c)) = named.first() {
        return Matched {
            by: By::Name,
            client: Some(Candidate::of(c)),
            candidates: named.iter().map(|(_, c)| Candidate::of(c)).collect(),
            note: if named.len() > 1 {
                format!("Name only ({} clients of that name)", named.len())
            } else {
                "Name only: tick to use".into()
            },
            ..base
        };
    }
    Matched {
        note: "No client found".into(),
        ..base
    }
}

/// A client's organizer link as the portal has it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organizer {
    pub id: String,
    pub client_id: String,
    pub tax_year: i32,
    #[serde(default)]
    pub return_type: String,
    pub url: String,
    pub status: Status,
    #[serde(default)]
    pub delivered: bool,
    /// Why its card is not shown: "delivery", "delivered", "off"; None while it is.
    pub off: Option<String>,
    pub off_at: Option<String>,
    pub off_by: Option<String>,
    pub delivery_at: Option<String>,
    pub imported_at: String,
    #[serde(default)]
    pub imported_by: String,
}

impl Organizer {
    /// "Showing", or why not.
    pub fn card_words(&self) -> String {
        match self.off.as_deref() {
            None => {
                if self.status == Status::Submitted {
                    "Showing (thanks)".into()
                } else {
                    "Showing".into()
                }
            }
            Some("delivery") => "Off: delivery started".into(),
            Some("delivered") => "Off: delivered".into(),
            Some(_) => format!(
                "Off: turned off{}",
                self.off_by
                    .as_deref()
                    .map(|b| format!(" by {b}"))
                    .unwrap_or_default()
            ),
        }
    }
}

/// What the portal did with an import.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct PutResult {
    pub added: u32,
    pub changed: u32,
    pub same: u32,
    /// People told their organizer is ready (owner Q&A 2026-11-25: once, a new link).
    #[serde(default)]
    pub told: u32,
    /// New links' clients with nobody on the portal yet (their card waits).
    #[serde(default)]
    pub nobody: u32,
    #[serde(default)]
    pub refused: Vec<Refused>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refused {
    pub client_id: String,
    pub why: String,
}

impl Portal {
    /// The organizer links of a tax year (and of one client).
    pub fn organizers(
        &self,
        tax_year: Option<i32>,
        client_id: Option<&str>,
    ) -> Result<Vec<Organizer>> {
        #[derive(Deserialize)]
        struct R {
            organizers: Vec<Organizer>,
        }
        let year = tax_year.map(|y| y.to_string());
        let mut q: Vec<(&str, &str)> = Vec::new();
        if let Some(y) = year.as_deref() {
            q.push(("taxYear", y));
        }
        if let Some(c) = client_id {
            q.push(("clientId", c));
        }
        self.get::<R>("/organizers", &q).map(|r| r.organizers)
    }

    /// Sends the matched rows of an export (a card turned off stays off); with `notify`, each
    /// new link whose card shows is told to the client's people once.
    pub fn put_organizers(
        &self,
        tax_year: i32,
        rows: &[(String, &ExportRow)],
        notify: bool,
    ) -> Result<PutResult> {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(client_id, r)| {
                serde_json::json!({
                    "clientId": client_id,
                    "url": r.url,
                    // The portal takes 20 characters (QA 2026-11-25: one longer value refused
                    // the whole import).
                    "returnType": r.return_type.trim().chars().take(20).collect::<String>(),
                    "status": r.status,
                    "delivered": r.delivered,
                })
            })
            .collect();
        self.put_json_large(
            "/organizers",
            &serde_json::json!({ "taxYear": tax_year, "items": items, "notify": notify }),
        )
    }

    /// Turns cards off, or on again.
    pub fn set_organizers_off(&self, ids: &[String], off: bool) -> Result<u32> {
        #[derive(Deserialize)]
        struct R {
            changed: u32,
        }
        self.send_json::<R>(
            "PATCH",
            "/organizers",
            &serde_json::json!({ "ids": ids, "off": off }),
        )
        .map(|r| r.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_fs::contacts::{Invitee, Planned};

    const LINK: &str = "https://clients.stanfordtax.com/aaa-bbb-ccccc-dd/organizer/0000";

    #[test]
    fn one_shared_word_does_not_tick_and_an_empty_id_matches_nothing() {
        let dir = vec![
            client("c1", "2001", "Smith Plumbing LLC", &["john@example.test"]),
            client("c2", "", "Pat Example", &[]),
        ];
        let m = match_rows(&[row("John Smith", &["john@example.test"], "")], &dir, 2025);
        assert!(!m[0].ticked, "{:?}", m[0].by);
        assert_ne!(m[0].by, By::EmailAndName);
        let m = match_rows(&[row("Jordan Sample", &[], "0")], &dir, 2025);
        assert_ne!(m[0].by, By::Id);
        assert!(m[0].client.is_none());
    }

    fn client(id: &str, code: &str, name: &str, emails: &[&str]) -> ClientContacts {
        ClientContacts {
            client_id: id.into(),
            code: code.into(),
            name: name.into(),
            partner: None,
            planned: Planned {
                people: emails
                    .iter()
                    .map(|e| Invitee {
                        name: name.into(),
                        email: Some((*e).into()),
                        phone: None,
                        role: "Taxpayer".into(),
                        note: None,
                    })
                    .collect(),
                problems: vec![],
            },
            names: vec![],
        }
    }

    fn row(client: &str, emails: &[&str], source_id: &str) -> ExportRow {
        ExportRow {
            line: 2,
            client: client.into(),
            emails: emails.iter().map(|e| (*e).into()).collect(),
            tax_year: Some(2025),
            return_type: "1040".into(),
            status: Status::InProgress,
            url: LINK.into(),
            source_id: source_id.into(),
            delivered: false,
        }
    }

    fn dir() -> Vec<ClientContacts> {
        vec![
            client(
                "u1",
                "1042",
                "Sample, Jordan & Alex",
                &["jordan@example.test"],
            ),
            client(
                "u2",
                "1043",
                "Sample Farms LLC",
                &["farm@example.test", "jordan@example.test"],
            ),
            client("u3", "1077", "Example, Pat & Lee", &[]),
            client("u4", "1103", "Doe, Chris", &["chris@example.test"]),
        ]
    }

    #[test]
    fn reads_the_export_by_its_column_names() {
        let csv = "\u{feff}ID,Emails,Client,Assignees,Salutation,Entity,Tax year,Return type,\
Questionnaire status,Questionnaire link,Tags,Source,Source id,Tax return delivery status\n\
x1,\"a@example.test, b@example.test\",\"Sample, Jordan & Alex\",s@firm.test,Jordan,,2025,1040,\
In progress,https://clients.stanfordtax.com/x/organizer/1,,ultratax,1042,Not delivered\n\
x2,,Demo LLC,s@firm.test,,Demo LLC,2025,1120s,Submitted,https://clients.stanfordtax.com/x/2,,,,Delivered\n";
        let rows = parse_export(csv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].emails, ["a@example.test", "b@example.test"]);
        assert_eq!(
            (
                rows[0].tax_year,
                rows[0].status,
                rows[0].source_id.as_str(),
                rows[0].delivered
            ),
            (Some(2025), Status::InProgress, "1042", false)
        );
        assert_eq!(
            (rows[1].status, rows[1].delivered, rows[1].line),
            (Status::Submitted, true, 3)
        );
        assert!(parse_export(b"Name,Email\nx,y\n").is_err());
        let named = "Client,Emails,Questionnaire link\n\
\"Doe, Chris\",\"Chris Doe <Chris@Example.test>; (pat@example.test)\",https://clients.stanfordtax.com/x\n";
        let rows = parse_export(named.as_bytes()).unwrap();
        assert_eq!(rows[0].emails, ["chris@example.test", "pat@example.test"]);
    }

    #[test]
    fn the_surest_match_first_and_a_name_alone_is_not_ticked() {
        let d = dir();
        let rows = vec![
            // ID.
            row("Sample, Jordan & Alex", &[], "1042"),
            // Email and name.
            row("Chris Doe", &["chris@example.test"], ""),
            // ID, but the email is another client's.
            row("Sample, Jordan", &["farm@example.test"], "1042"),
            // The email is on two clients.
            row("Sample, Jordan & Alex", &["jordan@example.test"], ""),
            // Name alone.
            row("Pat & Lee Example", &[], ""),
            // Nothing.
            row("Nobody, Here", &[], ""),
        ];
        let m = match_rows(&rows, &d, 2025);
        let got: Vec<(By, Option<&str>, bool)> = m
            .iter()
            .map(|m| (m.by, m.client.as_ref().map(|c| c.code.as_str()), m.ticked))
            .collect();
        assert_eq!(
            got,
            [
                (By::Id, Some("1042"), true),
                (By::EmailAndName, Some("1103"), true),
                (By::Check, Some("1042"), false),
                (By::Check, Some("1042"), false),
                (By::Name, Some("1077"), false),
                (By::None, None, false),
            ]
        );
        assert_eq!(m[3].candidates.len(), 2);
        assert_eq!(
            m[2].note,
            "ID 1042 is Sample, Jordan & Alex; the email is Sample Farms LLC (1043)'s"
        );
    }

    #[test]
    fn only_organizer_links_of_the_year_are_sent() {
        let d = dir();
        let mut bad = row("Sample, Jordan & Alex", &[], "1042");
        bad.url = "https://evil.example.com/clients.stanfordtax.com".into();
        let mut other_year = row("Doe, Chris", &[], "1103");
        other_year.tax_year = Some(2024);
        let m = match_rows(&[bad, other_year], &d, 2025);
        assert!(m.iter().all(|m| m.refused && !m.ticked));
        assert!(!allowed_link("http://clients.stanfordtax.com/x"));
        assert!(!allowed_link("https://clients.stanfordtax.com.evil.test/x"));
        assert!(allowed_link(LINK));
    }
}
