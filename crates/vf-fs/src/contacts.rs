//! Portal contacts from the client CSV (PA-105, owner 2026-11-24).
//!
//! The firm's client CSV (the one Intake › Setup and Admin's import read) may carry each
//! client's email and cell, and the spouse's. Which columns hold them is saved in
//! `clients.vf.json` ([`ContactColumns`]); the values are read from the CSV each time they are
//! needed, never copied into the cabinet, so a CSV updated by the tax software is what counts.
//! Nothing here talks to the portal: [`people_of`] says who would be invited, and Admin ›
//! Portal › Invite from client list compares that with the portal.

use std::path::Path;

use filer_core::clients::read_csv;
use serde::{Deserialize, Serialize};

use crate::clients::{load, master_dir, partner_of, save_meta};

/// The client CSV's columns holding portal contacts; `None`: not in this CSV.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactColumns {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spouse_first: Option<String>,
    /// Blank: the taxpayer's last name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spouse_last: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spouse_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spouse_cell: Option<String>,
}

impl ContactColumns {
    /// Every column chosen, for keeping them out of the client list's own columns.
    pub fn chosen(&self) -> Vec<&str> {
        [
            &self.first,
            &self.last,
            &self.email,
            &self.cell,
            &self.spouse_first,
            &self.spouse_last,
            &self.spouse_email,
            &self.spouse_cell,
        ]
        .into_iter()
        .filter_map(|c| c.as_deref())
        .collect()
    }

    /// An email or a cell column is chosen: there is something to invite with.
    pub fn any(&self) -> bool {
        [
            &self.email,
            &self.cell,
            &self.spouse_email,
            &self.spouse_cell,
        ]
        .iter()
        .any(|c| c.is_some())
    }

    /// Usual header names, for a CSV read the first time. A column already used as the
    /// client ID or name is never guessed.
    pub fn guess(headers: &[String], taken: &[&str]) -> Self {
        let key = |h: &str| -> String {
            h.to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect()
        };
        let find = |names: &[&str]| {
            headers
                .iter()
                .filter(|h| !taken.contains(&h.as_str()))
                .find(|h| names.contains(&key(h).as_str()))
                .cloned()
        };
        ContactColumns {
            first: find(&[
                "firstname",
                "tpfirstname",
                "tpfirst",
                "taxpayerfirstname",
                "first",
            ]),
            last: find(&[
                "lastname",
                "tplastname",
                "tplast",
                "taxpayerlastname",
                "last",
            ]),
            email: find(&[
                "email",
                "emailaddress",
                "tpemail",
                "taxpayeremail",
                "primaryemail",
            ]),
            cell: find(&[
                "cell",
                "cellphone",
                "mobile",
                "mobilephone",
                "tpcell",
                "tpcellphone",
                "tpmobile",
                "taxpayercell",
                "taxpayercellphone",
                "taxpayermobile",
            ]),
            spouse_first: find(&["spousefirstname", "spfirstname", "spfirst", "spousefirst"]),
            spouse_last: find(&["spouselastname", "splastname", "splast", "spouselast"]),
            spouse_email: find(&["spouseemail", "spemail", "spouseemailaddress"]),
            spouse_cell: find(&[
                "spousecell",
                "spousecellphone",
                "spousemobile",
                "spcell",
                "spcellphone",
                "spmobile",
            ]),
        }
    }
}

/// The saved contact columns (none saved: all `None`).
pub fn columns(root: &Path) -> Result<ContactColumns, String> {
    Ok(load(root)?.meta.contact_columns.unwrap_or_default())
}

/// Saves the contact columns (all `None`: removed).
pub fn set_columns(root: &Path, cols: &ContactColumns) -> Result<(), String> {
    crate::lock::with(root, "clients", "the client list", || {
        let mut m = load(root)?;
        m.meta.contact_columns = (cols != &ContactColumns::default()).then(|| cols.clone());
        save_meta(root, &m.meta)
    })
}

/// An email as the portal stores it (trimmed, lowercase), or `None` when it is not one.
pub fn normal_email(s: &str) -> Option<String> {
    let e = s.trim().to_lowercase();
    let (user, domain) = e.split_once('@')?;
    let ok = !user.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !e.chars().any(char::is_whitespace);
    ok.then_some(e)
}

/// A phone as the portal stores it (E.164, US numbers by default), or `None`.
pub fn normal_phone(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if digits.len() == 10 {
        Some(format!("+1{digits}"))
    } else if (digits.len() == 11 && digits.starts_with('1'))
        || (s.trim().starts_with('+') && digits.len() >= 10)
    {
        Some(format!("+{digits}"))
    } else {
        None
    }
}

/// Someone the CSV says to invite to a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitee {
    pub name: String,
    /// Normalised; the invitation goes here when there is one.
    pub email: Option<String>,
    /// Normalised; by text when there is no email, else kept for them to confirm.
    pub phone: Option<String>,
    pub role: String,
    /// Said beside them in the list (a contact that was not used, a shared email).
    pub note: Option<String>,
}

impl Invitee {
    /// How the invitation goes out.
    pub fn by(&self) -> String {
        match (&self.email, &self.phone) {
            (Some(e), _) => e.clone(),
            (None, Some(p)) => format!("text {p}"),
            (None, None) => String::new(),
        }
    }
}

/// One client's row of the CSV, as far as contacts go.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub first: String,
    pub last: String,
    pub email: String,
    pub cell: String,
    pub spouse_first: String,
    pub spouse_last: String,
    pub spouse_email: String,
    pub spouse_cell: String,
}

/// Entity types invited as one contact with the role Owner (no spouse).
fn is_business(entity: Option<&str>) -> bool {
    matches!(
        entity,
        Some("partnership" | "s_corp" | "c_corp" | "llc" | "trust" | "estate" | "nonprofit")
    )
}

fn join(a: &str, b: &str) -> String {
    format!("{} {}", a.trim(), b.trim()).trim().to_string()
}

/// One person's contact: (email, phone, what was not used).
fn contact(email: &str, cell: &str) -> (Option<String>, Option<String>, Option<String>) {
    let e = normal_email(email);
    let p = normal_phone(cell);
    let mut bad = Vec::new();
    if e.is_none() && !email.trim().is_empty() {
        bad.push(format!(
            "email \"{}\" is not an email address",
            email.trim()
        ));
    }
    if p.is_none() && !cell.trim().is_empty() {
        bad.push(format!("cell \"{}\" is not a phone number", cell.trim()));
    }
    (e, p, (!bad.is_empty()).then(|| bad.join("; ")))
}

/// Who the CSV row says to invite to the client (owner 2026-11-24):
///
/// - the taxpayer (role Taxpayer; Owner for a business entity type), named from the first
///   and last name columns, else the client's name;
/// - for a person (not a business), the spouse when the spouse has an email or cell (role
///   Spouse; their last name defaults to the taxpayer's);
/// - a couple sharing one email, or with no email and one cell: one invitation in both names,
///   role "Taxpayer & Spouse";
/// - a couple with two emails and one cell: each by email, the cell kept with the taxpayer only
///   (a phone signs in to one portal account).
///
/// Someone with no usable email or cell is left out; [`Planned::problems`] says why.
pub fn people_of(row: &Row, client_name: &str, entity: Option<&str>) -> Planned {
    let mut out = Planned::default();
    let business = is_business(entity);
    let tp_name = {
        let n = join(&row.first, &row.last);
        if n.is_empty() {
            client_name.trim().to_string()
        } else {
            n
        }
    };
    let (te, tp, tbad) = contact(&row.email, &row.cell);
    let mut taxpayer = (te.is_some() || tp.is_some()).then(|| Invitee {
        name: tp_name.clone(),
        email: te.clone(),
        phone: tp.clone(),
        role: if business { "Owner" } else { "Taxpayer" }.into(),
        note: tbad.clone(),
    });
    if taxpayer.is_none() {
        out.problems.push(match tbad {
            Some(b) => format!("{tp_name}: {b}"),
            None => format!("{tp_name}: no email or cell"),
        });
    }
    let has_spouse = !business
        && [&row.spouse_email, &row.spouse_cell]
            .iter()
            .any(|v| !v.trim().is_empty());
    let mut spouse = None;
    if has_spouse {
        let last = if row.spouse_last.trim().is_empty() {
            &row.last
        } else {
            &row.spouse_last
        };
        let first = row.spouse_first.trim();
        let name = if first.is_empty() {
            "Spouse".to_string()
        } else {
            join(first, last)
        };
        let (se, sp, sbad) = contact(&row.spouse_email, &row.spouse_cell);
        if se.is_none() && sp.is_none() {
            out.problems.push(format!(
                "{name}: {}",
                sbad.unwrap_or_else(|| "no email or cell".into())
            ));
        } else {
            spouse = Some(Invitee {
                name,
                email: se,
                phone: sp,
                role: "Spouse".into(),
                note: sbad,
            });
        }
    }
    match (&mut taxpayer, spouse) {
        (Some(t), Some(mut s)) => {
            let same_email = t.email.is_some() && t.email == s.email;
            let same_cell_only =
                t.email.is_none() && s.email.is_none() && t.phone.is_some() && t.phone == s.phone;
            if same_email || same_cell_only {
                // One login for the couple.
                let sf = row.spouse_first.trim();
                t.name = if sf.is_empty() {
                    t.name.clone()
                } else if row.spouse_last.trim().is_empty()
                    || row.spouse_last.trim().eq_ignore_ascii_case(row.last.trim())
                {
                    join(&format!("{} & {sf}", row.first.trim()), &row.last)
                } else {
                    format!("{} & {}", t.name, s.name)
                };
                t.role = "Taxpayer & Spouse".into();
                if t.phone.is_none() {
                    t.phone = s.phone.take();
                }
                t.note = Some(
                    [
                        t.note.take(),
                        s.note.take(),
                        Some(if same_email {
                            "shared email: one invitation for both".into()
                        } else {
                            "shared cell: one invitation for both".into()
                        }),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; "),
                );
                out.people.push(t.clone());
            } else {
                if t.phone.is_some() && t.phone == s.phone {
                    // Both have an email: the cell stays with the taxpayer.
                    s.phone = None;
                }
                out.people.push(t.clone());
                out.people.push(s);
            }
        }
        (Some(t), None) => out.people.push(t.clone()),
        (None, Some(s)) => out.people.push(s),
        (None, None) => {}
    }
    out
}

/// Who to invite for one client, and why anyone was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Planned {
    pub people: Vec<Invitee>,
    pub problems: Vec<String>,
}

/// One client of the cabinet with what the CSV says about its portal contacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientContacts {
    /// The cabinet's client id (the portal's client id).
    pub client_id: String,
    /// The CSV ID (the client code).
    pub code: String,
    pub name: String,
    pub partner: Option<String>,
    pub planned: Planned,
}

/// Reads the client CSV Intake › Setup names, with the saved contact columns: every client of
/// the client list the CSV has a row for, in CSV order. Errors in plain words (no CSV, no
/// contact columns, the CSV unreadable).
pub fn read(root: &Path) -> Result<Vec<ClientContacts>, String> {
    let master = load(root)?;
    let cols = master.meta.contact_columns.clone().unwrap_or_default();
    if !cols.any() {
        return Err(
            "No email or cell column is chosen yet: choose them under Portal contacts in Import clients CSV…"
                .into(),
        );
    }
    let list = master
        .list
        .as_ref()
        .ok_or("No client list is imported yet: use Import clients CSV… first.")?;
    let (settings, _) = filer_core::config::Settings::load_or_default(&master_dir(root));
    let csv = settings
        .csv_path
        .clone()
        .ok_or("No client CSV is set: use Import clients CSV… first.")?;
    let table = read_csv(&csv).map_err(|e| format!("{}: {e}", csv.display()))?;
    let col = |h: &Option<String>| -> Result<Option<usize>, String> {
        match h {
            None => Ok(None),
            Some(h) => table
                .headers
                .iter()
                .position(|x| x == h)
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "The client CSV has no column \"{h}\" any more: choose the portal contact columns again in Import clients CSV…"
                    )
                }),
        }
    };
    let id_col = col(&settings.column_map.id)?.ok_or("The client CSV has no ID column set.")?;
    let c = [
        col(&cols.first)?,
        col(&cols.last)?,
        col(&cols.email)?,
        col(&cols.cell)?,
        col(&cols.spouse_first)?,
        col(&cols.spouse_last)?,
        col(&cols.spouse_email)?,
        col(&cols.spouse_cell)?,
    ];
    let mut out = Vec::new();
    for r in &table.rows {
        let Some(client) = r.get(id_col).and_then(|id| list.match_by_id(id.trim())) else {
            continue;
        };
        let v = |i: Option<usize>| i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
        let row = Row {
            first: v(c[0]),
            last: v(c[1]),
            email: v(c[2]),
            cell: v(c[3]),
            spouse_first: v(c[4]),
            spouse_last: v(c[5]),
            spouse_email: v(c[6]),
            spouse_cell: v(c[7]),
        };
        let id = client.uuid.to_string();
        let entity = master.meta.entity_types.get(&id).map(String::as_str);
        out.push(ClientContacts {
            planned: people_of(&row, &client.name, entity),
            partner: partner_of(&master.meta, &id, Some(client)),
            client_id: id,
            code: client.id.clone(),
            name: client.name.clone(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(f: &[(&str, &str)]) -> Row {
        let mut r = Row::default();
        for (k, v) in f {
            let v = v.to_string();
            match *k {
                "first" => r.first = v,
                "last" => r.last = v,
                "email" => r.email = v,
                "cell" => r.cell = v,
                "sfirst" => r.spouse_first = v,
                "slast" => r.spouse_last = v,
                "semail" => r.spouse_email = v,
                "scell" => r.spouse_cell = v,
                _ => panic!("{k}"),
            }
        }
        r
    }

    fn who(p: &Planned) -> Vec<(String, String, Option<String>, String)> {
        p.people
            .iter()
            .map(|i| (i.name.clone(), i.by(), i.phone.clone(), i.role.clone()))
            .collect()
    }

    #[test]
    fn contacts_are_normalised_as_the_portal_does() {
        assert_eq!(
            normal_email(" Jane@Example.TEST "),
            Some("jane@example.test".into())
        );
        assert_eq!(normal_email("jane@example"), None);
        assert_eq!(normal_email("jane example@x.test"), None);
        assert_eq!(normal_phone("(555) 201-0002"), Some("+15552010002".into()));
        assert_eq!(normal_phone("1-555-201-0002"), Some("+15552010002".into()));
        assert_eq!(
            normal_phone("+44 20 7946 0958"),
            Some("+442079460958".into())
        );
        assert_eq!(normal_phone("201-0002"), None);
    }

    #[test]
    fn a_joint_couple_gets_two_invitations() {
        let p = people_of(
            &row(&[
                ("first", "John"),
                ("last", "Sample"),
                ("email", "john@example.test"),
                ("cell", "555-201-0001"),
                ("sfirst", "Jane"),
                ("semail", "jane@example.test"),
                ("scell", "555-201-0002"),
            ]),
            "Sample, John & Jane",
            None,
        );
        assert_eq!(
            who(&p),
            vec![
                (
                    "John Sample".into(),
                    "john@example.test".into(),
                    Some("+15552010001".into()),
                    "Taxpayer".into()
                ),
                (
                    "Jane Sample".into(),
                    "jane@example.test".into(),
                    Some("+15552010002".into()),
                    "Spouse".into()
                ),
            ]
        );
        assert!(p.problems.is_empty());
    }

    #[test]
    fn a_shared_email_is_one_invitation_in_both_names() {
        let p = people_of(
            &row(&[
                ("first", "John"),
                ("last", "Sample"),
                ("email", "family@example.test"),
                ("sfirst", "Jane"),
                ("semail", "Family@example.test"),
                ("scell", "555-201-0002"),
            ]),
            "Sample, John & Jane",
            Some("individual"),
        );
        assert_eq!(
            who(&p),
            vec![(
                "John & Jane Sample".into(),
                "family@example.test".into(),
                Some("+15552010002".into()),
                "Taxpayer & Spouse".into()
            )]
        );
        assert!(
            p.people[0]
                .note
                .as_deref()
                .unwrap()
                .contains("shared email")
        );
    }

    #[test]
    fn two_emails_and_one_cell_keep_the_cell_with_the_taxpayer() {
        let p = people_of(
            &row(&[
                ("first", "John"),
                ("last", "Sample"),
                ("email", "john@example.test"),
                ("cell", "555 201 0001"),
                ("sfirst", "Jane"),
                ("slast", "Doe"),
                ("semail", "jane@example.test"),
                ("scell", "5552010001"),
            ]),
            "Sample, John & Jane",
            None,
        );
        assert_eq!(p.people.len(), 2);
        assert_eq!(p.people[0].phone.as_deref(), Some("+15552010001"));
        assert_eq!(p.people[1].name, "Jane Doe");
        assert_eq!(p.people[1].phone, None);
    }

    #[test]
    fn a_cell_only_person_is_invited_by_text_and_a_bad_email_is_said() {
        let p = people_of(
            &row(&[("email", "not an email"), ("cell", "555-201-0003")]),
            "Doe, Alex",
            None,
        );
        assert_eq!(
            who(&p),
            vec![(
                "Doe, Alex".into(),
                "text +15552010003".into(),
                Some("+15552010003".into()),
                "Taxpayer".into()
            )]
        );
        assert!(
            p.people[0]
                .note
                .as_deref()
                .unwrap()
                .contains("not an email address")
        );
        let none = people_of(&row(&[("email", "nope")]), "Doe, Alex", None);
        assert!(none.people.is_empty());
        assert_eq!(
            none.problems,
            vec!["Doe, Alex: email \"nope\" is not an email address".to_string()]
        );
    }

    #[test]
    fn a_business_invites_its_contact_as_owner_and_no_spouse() {
        let p = people_of(
            &row(&[
                ("first", "Pat"),
                ("last", "Sample"),
                ("email", "pat@example.test"),
                ("semail", "other@example.test"),
            ]),
            "Sample Farms LLC",
            Some("s_corp"),
        );
        assert_eq!(
            who(&p),
            vec![(
                "Pat Sample".into(),
                "pat@example.test".into(),
                None,
                "Owner".into()
            )]
        );
    }

    #[test]
    fn usual_headers_are_guessed() {
        let h: Vec<String> = [
            "Client ID",
            "Client Name",
            "TP First Name",
            "TP Last Name",
            "TP Email",
            "TP Cell Phone",
            "SP First Name",
            "Spouse Email",
            "SP Cell",
        ]
        .map(String::from)
        .to_vec();
        let g = ContactColumns::guess(&h, &["Client ID", "Client Name"]);
        assert_eq!(g.first.as_deref(), Some("TP First Name"));
        assert_eq!(g.last.as_deref(), Some("TP Last Name"));
        assert_eq!(g.email.as_deref(), Some("TP Email"));
        assert_eq!(g.cell.as_deref(), Some("TP Cell Phone"));
        assert_eq!(g.spouse_first.as_deref(), Some("SP First Name"));
        assert_eq!(g.spouse_last, None);
        assert_eq!(g.spouse_email.as_deref(), Some("Spouse Email"));
        assert_eq!(g.spouse_cell.as_deref(), Some("SP Cell"));
    }

    /// The CSV is read each time: a changed email in it is what the list shows.
    #[test]
    fn read_follows_the_csv() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(master_dir(root)).unwrap();
        let csv = root.join("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name,Email,Cell\n1001,\"Sample, John\",john@example.test,\n1002,Sample Farms LLC,,555-201-0003\n",
        )
        .unwrap();
        let map = filer_core::config::ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            partner: None,
            extras: vec![],
        };
        crate::clients::import_csv_tin(root, &csv, &map, &Default::default(), None).unwrap();
        crate::clients::use_csv_in_intake(root, &csv, &map).unwrap();
        assert!(read(root).unwrap_err().contains("No email or cell column"));
        set_columns(
            root,
            &ContactColumns {
                email: Some("Email".into()),
                cell: Some("Cell".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let got = read(root).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].code, "1001");
        assert_eq!(got[0].planned.people[0].by(), "john@example.test");
        assert_eq!(got[1].planned.people[0].by(), "text +15552010003");
        std::fs::write(
            &csv,
            "ID,Name,Email,Cell\n1001,\"Sample, John\",john.new@example.test,\n",
        )
        .unwrap();
        let got = read(root).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].planned.people[0].by(), "john.new@example.test");
    }
}
