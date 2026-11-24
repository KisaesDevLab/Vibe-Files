//! Portal contacts of each client (PA-105, owner 2026-11-24).
//!
//! The taxpayer's and spouse's name, email and cell, kept in `clients.vf.json` by client uuid
//! ([`ContactRecord`]): what the client CSV said at the last **Import** (its columns chosen
//! under Portal contacts, [`ContactColumns`]) and what staff typed in the client panel, which
//! wins over the CSV until cleared (owner 2026-11-24: "Stored in _Vibe", "hand edits win").
//! The CSV is not needed after the import. Intake's own re-reads of the CSV never change them:
//! only Import does, after showing what changes ([`preview`]).
//!
//! Nothing here talks to the portal: [`people_of`] says who would be invited, and Admin ›
//! Portal › Invite from client list compares that with the portal.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use filer_core::clients::{ClientList, CsvTable};
use filer_core::config::ColumnMap;
use serde::{Deserialize, Serialize};

use crate::clients::{load, partner_of, save_meta};

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

/// One client's contacts: as the CSV gave them, or as they are after hand edits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cell: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spouse_first: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spouse_last: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spouse_email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub spouse_cell: String,
}

/// The fields of a [`Row`]: key (as kept in `hand`) and the words for it.
pub const FIELDS: [(&str, &str); 8] = [
    ("first", "Taxpayer first name"),
    ("last", "Taxpayer last name"),
    ("email", "Email"),
    ("cell", "Cell phone"),
    ("spouse_first", "Spouse first name"),
    ("spouse_last", "Spouse last name"),
    ("spouse_email", "Spouse email"),
    ("spouse_cell", "Spouse cell"),
];

impl Row {
    pub fn get(&self, key: &str) -> &str {
        match key {
            "first" => &self.first,
            "last" => &self.last,
            "email" => &self.email,
            "cell" => &self.cell,
            "spouse_first" => &self.spouse_first,
            "spouse_last" => &self.spouse_last,
            "spouse_email" => &self.spouse_email,
            "spouse_cell" => &self.spouse_cell,
            _ => "",
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut String> {
        Some(match key {
            "first" => &mut self.first,
            "last" => &mut self.last,
            "email" => &mut self.email,
            "cell" => &mut self.cell,
            "spouse_first" => &mut self.spouse_first,
            "spouse_last" => &mut self.spouse_last,
            "spouse_email" => &mut self.spouse_email,
            "spouse_cell" => &mut self.spouse_cell,
            _ => return None,
        })
    }

    pub fn is_empty(&self) -> bool {
        FIELDS.iter().all(|(k, _)| self.get(k).trim().is_empty())
    }

    /// An email or cell for anyone.
    pub fn has_contact(&self) -> bool {
        ["email", "cell", "spouse_email", "spouse_cell"]
            .iter()
            .any(|k| !self.get(k).trim().is_empty())
    }
}

/// A client's contacts in `clients.vf.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRecord {
    /// What the client CSV said at the last Import.
    #[serde(default, skip_serializing_if = "Row::is_empty")]
    pub csv: Row,
    /// Fields staff typed (key from [`FIELDS`] → value; an empty value: cleared by hand). They
    /// win over `csv` until cleared.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hand: BTreeMap<String, String>,
}

impl ContactRecord {
    /// The contacts in use: each field typed by hand, else the CSV's.
    pub fn effective(&self) -> Row {
        let mut r = self.csv.clone();
        for (k, v) in &self.hand {
            if let Some(f) = r.get_mut(k) {
                *f = v.clone();
            }
        }
        r
    }

    pub fn is_empty(&self) -> bool {
        self.csv.is_empty() && self.hand.is_empty()
    }
}

/// A client's stored contacts (none: empty).
pub fn record(root: &Path, client_id: &str) -> Result<ContactRecord, String> {
    Ok(load(root)?
        .meta
        .contacts
        .get(client_id)
        .cloned()
        .unwrap_or_default())
}

/// Saves what staff typed for a client: each field of `row` that differs from the CSV's is kept
/// as typed by hand; a field equal to the CSV's goes back to following the CSV.
pub fn set_by_hand(root: &Path, client_id: &str, row: &Row) -> Result<ContactRecord, String> {
    crate::lock::with(root, "clients", "the client list", || {
        let mut m = load(root)?;
        let rec = m.meta.contacts.entry(client_id.to_string()).or_default();
        rec.hand.clear();
        for (k, _) in FIELDS {
            let v = row.get(k).trim();
            if v != rec.csv.get(k).trim() {
                rec.hand.insert(k.to_string(), v.to_string());
            }
        }
        let out = rec.clone();
        if out.is_empty() {
            m.meta.contacts.remove(client_id);
        }
        save_meta(root, &m.meta)?;
        Ok(out)
    })
}

/// Saves one field as typed by hand (Admin › Portal requests: the contact a person confirmed,
/// when approving them; PA-106). The other fields are left as they are.
pub fn set_one_by_hand(root: &Path, client_id: &str, key: &str, value: &str) -> Result<(), String> {
    if !FIELDS.iter().any(|(k, _)| *k == key) {
        return Err(format!("no contact field {key}"));
    }
    crate::lock::with(root, "clients", "the client list", || {
        let mut m = load(root)?;
        let rec = m.meta.contacts.entry(client_id.to_string()).or_default();
        if rec.csv.get(key).trim() == value.trim() {
            rec.hand.remove(key);
        } else {
            rec.hand.insert(key.to_string(), value.trim().to_string());
        }
        save_meta(root, &m.meta)
    })
}

/// Moves Admin › Portal's old email and mobile column choices (client-list token columns kept
/// in `clients.json`, PA-59) over to the Portal contacts, once (owner 2026-11-24: one place for
/// contact columns): each listed client's value becomes its CSV email or cell where it has
/// none, and the Portal contacts columns are set to those CSV columns when not chosen yet.
/// Answers how many clients got a contact.
pub fn adopt_record_columns(
    root: &Path,
    email_token: Option<&str>,
    mobile_token: Option<&str>,
) -> Result<usize, String> {
    crate::lock::with(root, "clients", "the client list", || {
        let mut m = load(root)?;
        let (settings, _) =
            filer_core::config::Settings::load_or_default(&crate::clients::master_dir(root));
        let header = |token: Option<&str>| -> Option<String> {
            let t = token?;
            settings
                .column_map
                .extras
                .iter()
                .find(|h| filer_core::clients::token_name(h) == t)
                .cloned()
        };
        let mut cols = m.meta.contact_columns.clone().unwrap_or_default();
        if cols.email.is_none() {
            cols.email = header(email_token);
        }
        if cols.cell.is_none() {
            cols.cell = header(mobile_token);
        }
        if cols != ContactColumns::default() {
            m.meta.contact_columns = Some(cols);
        }
        // The first address or number of a cell that may hold several.
        let first = |v: &str| {
            v.split([';', ',', '/', '|'])
                .map(str::trim)
                .find(|x| !x.is_empty())
                .unwrap_or("")
                .to_string()
        };
        let mut n = 0;
        if let Some(list) = &m.list {
            for c in &list.clients {
                let key = c.uuid.to_string();
                let mut got = false;
                for (token, field) in [(email_token, "email"), (mobile_token, "cell")] {
                    let Some(v) = token.and_then(|t| c.extra.get(t)).map(|v| first(v)) else {
                        continue;
                    };
                    if v.is_empty() {
                        continue;
                    }
                    let rec = m.meta.contacts.entry(key.clone()).or_default();
                    if let Some(f) = rec.csv.get_mut(field)
                        && f.trim().is_empty()
                    {
                        *f = v;
                        got = true;
                    }
                }
                if got {
                    n += 1;
                }
            }
        }
        save_meta(root, &m.meta)?;
        Ok(n)
    })
}

/// Drops a client's hand edits: its contacts follow the CSV again.
pub fn clear_hand(root: &Path, client_id: &str) -> Result<ContactRecord, String> {
    crate::lock::with(root, "clients", "the client list", || {
        let mut m = load(root)?;
        let mut out = ContactRecord::default();
        if let Some(rec) = m.meta.contacts.get_mut(client_id) {
            rec.hand.clear();
            out = rec.clone();
            if rec.is_empty() {
                m.meta.contacts.remove(client_id);
            }
        }
        save_meta(root, &m.meta)?;
        Ok(out)
    })
}

/// Each CSV row's contacts by its client ID (as written in the ID column), with `cols`.
pub fn csv_rows(
    table: &CsvTable,
    map: &ColumnMap,
    cols: &ContactColumns,
) -> Result<HashMap<String, Row>, String> {
    let col = |h: &Option<String>| -> Result<Option<usize>, String> {
        match h {
            None => Ok(None),
            Some(h) => table
                .headers
                .iter()
                .position(|x| x == h)
                .map(Some)
                .ok_or_else(|| format!("The client CSV has no column \"{h}\"")),
        }
    };
    let id_col = col(&map.id)?.ok_or("Choose the ID column.")?;
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
    let mut out = HashMap::new();
    for r in &table.rows {
        let Some(id) = r.get(id_col).map(|x| x.trim()).filter(|x| !x.is_empty()) else {
            continue;
        };
        let v = |i: Option<usize>| {
            i.and_then(|i| r.get(i))
                .map(|x| x.trim().to_string())
                .unwrap_or_default()
        };
        out.insert(
            id.to_string(),
            Row {
                first: v(c[0]),
                last: v(c[1]),
                email: v(c[2]),
                cell: v(c[3]),
                spouse_first: v(c[4]),
                spouse_last: v(c[5]),
                spouse_email: v(c[6]),
                spouse_cell: v(c[7]),
            },
        );
    }
    Ok(out)
}

/// Import: keeps each listed client's contacts as the CSV gives them (`rows` by CSV ID; a
/// client the CSV has no row for keeps what it had, should it come back). Hand edits stay. With no
/// contact columns chosen, nothing changes.
pub fn store_from_csv(
    root: &Path,
    list: &ClientList,
    rows: &HashMap<String, Row>,
) -> Result<usize, String> {
    crate::lock::with(root, "clients", "the client list", || {
        let mut m = load(root)?;
        let mut changed = 0;
        for c in &list.clients {
            let Some(row) = rows.get(c.id.trim()) else {
                continue;
            };
            let key = c.uuid.to_string();
            let rec = m.meta.contacts.entry(key.clone()).or_default();
            if rec.csv != *row {
                rec.csv = row.clone();
                changed += 1;
            }
            if rec.is_empty() {
                m.meta.contacts.remove(&key);
            }
        }
        save_meta(root, &m.meta)?;
        Ok(changed)
    })
}

/// What an Import would change, shown before it is applied (owner 2026-11-24).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    /// (ID, name) of clients the list does not have yet.
    pub new: Vec<(String, String)>,
    /// (ID, name) of listed clients this CSV has no row for: they leave the client list (their
    /// folders, documents and contacts stay).
    pub missing: Vec<(String, String)>,
    /// (ID, old name, new name).
    pub renamed: Vec<(String, String, String)>,
    pub contacts: Vec<ContactChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactChange {
    pub id: String,
    pub client: String,
    /// The field's words ([`FIELDS`]).
    pub field: &'static str,
    pub old: String,
    pub new: String,
    /// Typed by hand: the hand value is kept, the CSV's is only recorded.
    pub kept_by_hand: bool,
}

impl Preview {
    pub fn is_empty(&self) -> bool {
        self.new.is_empty()
            && self.missing.is_empty()
            && self.renamed.is_empty()
            && self.contacts.is_empty()
    }
}

/// What importing `table` with `map` (and contact columns `cols`, when any) would change in the
/// cabinet's client list and contacts. `list` and `stored` are the cabinet's now.
pub fn preview(
    table: &CsvTable,
    map: &ColumnMap,
    cols: &ContactColumns,
    list: Option<&ClientList>,
    stored: &BTreeMap<String, ContactRecord>,
) -> Result<Preview, String> {
    let id_col = map
        .id
        .as_ref()
        .and_then(|h| table.headers.iter().position(|x| x == h))
        .ok_or("Choose the ID column.")?;
    let name_col = map
        .name
        .as_ref()
        .and_then(|h| table.headers.iter().position(|x| x == h))
        .ok_or("Choose the name column.")?;
    let mut out = Preview::default();
    let mut seen = std::collections::HashSet::new();
    for r in &table.rows {
        let (Some(id), Some(name)) = (r.get(id_col), r.get(name_col)) else {
            continue;
        };
        let (id, name) = (id.trim(), name.trim());
        if id.is_empty() || name.is_empty() {
            continue;
        }
        seen.insert(id.to_string());
        match list.and_then(|l| l.match_by_id(id)) {
            None => out.new.push((id.into(), name.into())),
            Some(c) if c.name.trim() != name => {
                out.renamed
                    .push((id.into(), c.name.trim().into(), name.into()))
            }
            Some(_) => {}
        }
    }
    if let Some(l) = list {
        for c in &l.clients {
            if !seen.contains(c.id.trim()) {
                out.missing.push((c.id.clone(), c.name.clone()));
            }
        }
    }
    if cols.any() || !cols.chosen().is_empty() {
        let rows = csv_rows(table, map, cols)?;
        let mut ids: Vec<&String> = rows.keys().collect();
        ids.sort();
        for id in ids {
            let row = &rows[id];
            let (client, rec) = match list.and_then(|l| l.match_by_id(id)) {
                Some(c) => (
                    c.name.clone(),
                    stored.get(&c.uuid.to_string()).cloned().unwrap_or_default(),
                ),
                // A new client: its contacts are all new.
                None => (String::new(), ContactRecord::default()),
            };
            for (k, words) in FIELDS {
                let (old, new) = (rec.csv.get(k).trim(), row.get(k).trim());
                if old != new {
                    out.contacts.push(ContactChange {
                        id: id.clone(),
                        client: client.clone(),
                        field: words,
                        old: old.into(),
                        new: new.into(),
                        kept_by_hand: rec.hand.contains_key(k),
                    });
                }
            }
        }
    }
    Ok(out)
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
    // Without name columns, a person's client name gives the names ("Sample, John & Jane" is
    // John Sample and Jane Sample); a business keeps its own name.
    let from_client = if business {
        Vec::new()
    } else {
        names_in(client_name)
    };
    let tp_name = {
        let n = join(&row.first, &row.last);
        if !n.is_empty() {
            n
        } else if let Some(n) = from_client.first() {
            n.clone()
        } else {
            client_name.trim().to_string()
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
            from_client
                .get(1)
                .cloned()
                .unwrap_or_else(|| "Spouse".to_string())
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
                    // The cell stays with the taxpayer (a phone signs in to one account).
                    s.phone = None;
                }
                out.people.push(t.clone());
                if s.email.is_some() || s.phone.is_some() {
                    out.people.push(s);
                } else {
                    // Only the taxpayer's own cell was given for them (review 2026-11-24).
                    out.problems.push(format!(
                        "{}: only the taxpayer's cell, which signs in to the taxpayer's login",
                        s.name
                    ));
                }
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
    /// The people the client is, by name, for finding an access request by name (PA-106):
    /// (name, role, is the spouse). From the contacts' names, else the client's name
    /// ("Sample, John & Jane"); empty for a business with no names.
    pub names: Vec<(String, String, bool)>,
}

/// The people a client's name stands for: "Sample, John & Jane" is John Sample and Jane
/// Sample; "Chen, Wei & Lin Park" is Wei Chen and Lin Park. A name without a comma (a
/// business) stands for nobody.
fn names_in(client_name: &str) -> Vec<String> {
    let Some((last, firsts)) = client_name.split_once(',') else {
        return Vec::new();
    };
    let last = last.trim();
    firsts
        .split('&')
        .flat_map(|p| p.split(" and "))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            if p.split_whitespace().count() > 1 {
                p.to_string()
            } else {
                format!("{p} {last}")
            }
        })
        .collect()
}

/// Every client of the list with what is known of its people: its contacts (CSV or typed;
/// none: nobody to invite) and its names. In list order.
pub fn directory(root: &Path) -> Result<Vec<ClientContacts>, String> {
    let master = load(root)?;
    let list = master
        .list
        .as_ref()
        .ok_or("No client list is imported yet: use Import clients CSV… first.")?;
    let mut out = Vec::new();
    for client in &list.clients {
        let id = client.uuid.to_string();
        let row = master
            .meta
            .contacts
            .get(&id)
            .map(ContactRecord::effective)
            .unwrap_or_default();
        let entity = master.meta.entity_types.get(&id).map(String::as_str);
        let business = is_business(entity);
        let planned = if row.has_contact() {
            people_of(&row, &client.name, entity)
        } else {
            Planned::default()
        };
        let mut names = Vec::new();
        let tp = join(&row.first, &row.last);
        if !tp.is_empty() {
            names.push((
                tp,
                if business { "Owner" } else { "Taxpayer" }.to_string(),
                false,
            ));
        }
        if !business && !row.spouse_first.trim().is_empty() {
            let last = if row.spouse_last.trim().is_empty() {
                &row.last
            } else {
                &row.spouse_last
            };
            names.push((join(&row.spouse_first, last), "Spouse".into(), true));
        }
        if names.is_empty() && !business {
            for (i, n) in names_in(&client.name).into_iter().enumerate() {
                let spouse = i > 0;
                names.push((n, if spouse { "Spouse" } else { "Taxpayer" }.into(), spouse));
            }
        }
        out.push(ClientContacts {
            planned,
            names,
            partner: partner_of(&master.meta, &id, Some(client)),
            client_id: id,
            code: client.id.clone(),
            name: client.name.clone(),
        });
    }
    Ok(out)
}

/// Every client of the list with an email or cell (CSV or typed), in list order, and how many
/// clients have none. Errors in plain words.
pub fn read(root: &Path) -> Result<(Vec<ClientContacts>, usize), String> {
    let all = directory(root)?;
    let n = all.len();
    let out: Vec<ClientContacts> = all
        .into_iter()
        .filter(|c| !c.planned.people.is_empty() || !c.planned.problems.is_empty())
        .collect();
    let none = n - out.len();
    if out.is_empty() {
        return Err(
            "No client has an email or cell yet: choose the Portal contacts columns in Import clients CSV…, or type them in a client's panel (Portal contacts)."
                .into(),
        );
    }
    Ok((out, none))
}

/// How an access request was found to be a client's person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundBy {
    /// The email the person confirmed with a code is on file for them.
    Email,
    /// The cell they confirmed is on file for them.
    Cell,
    /// Only the name they typed is the name on file: not a match, a lead.
    Name,
}

/// A client an access request may be for (PA-106).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub client_id: String,
    pub code: String,
    pub client: String,
    /// The person's role at the client (Taxpayer, Spouse, Owner, Taxpayer & Spouse).
    pub role: String,
    /// The name on file for them.
    pub person: String,
    pub by: FoundBy,
    /// They are the spouse on file (where their contact would be saved).
    pub spouse: bool,
}

/// First and last word of a name, in lower case, letters only: "Jane Q. Sample" is
/// ("jane", "sample").
fn first_last(name: &str) -> Option<(String, String)> {
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphabetic() && c != '\'' && c != '-')
        .map(|w| w.trim_matches(['\'', '-']).to_lowercase())
        .filter(|w| w.chars().count() > 1)
        .collect();
    match words.as_slice() {
        [first, .., last] => Some((first.clone(), last.clone())),
        _ => None,
    }
}

/// The clients an access request is for: every client where the contact the person confirmed
/// (email or cell) is a person's on file. Only when none is, the clients where the name typed
/// is a person's on file (first and last name; middle names aside), as leads to check.
pub fn find(confirmed: &str, name: &str, dir: &[ClientContacts]) -> Vec<Found> {
    let email = normal_email(confirmed);
    let phone = if email.is_none() {
        normal_phone(confirmed)
    } else {
        None
    };
    let mut out = Vec::new();
    if email.is_some() || phone.is_some() {
        for c in dir {
            for p in &c.planned.people {
                let by = if email.is_some() && p.email == email {
                    FoundBy::Email
                } else if phone.is_some() && p.phone == phone {
                    FoundBy::Cell
                } else {
                    continue;
                };
                out.push(Found {
                    client_id: c.client_id.clone(),
                    code: c.code.clone(),
                    client: c.name.clone(),
                    role: p.role.clone(),
                    person: p.name.clone(),
                    by,
                    spouse: p.role == "Spouse",
                });
                break;
            }
        }
    }
    if !out.is_empty() {
        return out;
    }
    let Some(asked) = first_last(name) else {
        return out;
    };
    for c in dir {
        if let Some((n, role, spouse)) = c
            .names
            .iter()
            .find(|(n, _, _)| first_last(n).as_ref() == Some(&asked))
        {
            out.push(Found {
                client_id: c.client_id.clone(),
                code: c.code.clone(),
                client: c.name.clone(),
                role: role.clone(),
                person: n.clone(),
                by: FoundBy::Name,
                spouse: *spouse,
            });
        }
    }
    out
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

    /// The taxpayer has an email and a cell; the spouse only that same cell: the spouse is
    /// not listed with nothing to invite with (review 2026-11-24).
    #[test]
    fn a_spouse_with_only_the_taxpayers_cell_is_left_out() {
        let p = people_of(
            &row(&[
                ("first", "John"),
                ("last", "Sample"),
                ("email", "john@example.test"),
                ("cell", "555-201-0001"),
                ("sfirst", "Jane"),
                ("scell", "555-201-0001"),
            ]),
            "Sample, John & Jane",
            None,
        );
        assert_eq!(p.people.len(), 1);
        assert_eq!(p.people[0].phone.as_deref(), Some("+15552010001"));
        assert!(p.problems[0].starts_with("Jane Sample: only the taxpayer's cell"));
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
                "Alex Doe".into(),
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
            vec!["Alex Doe: email \"nope\" is not an email address".to_string()]
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

    fn dir_entry(id: &str, name: &str, row: Row, entity: Option<&str>) -> ClientContacts {
        let planned = if row.has_contact() {
            people_of(&row, name, entity)
        } else {
            Planned::default()
        };
        let mut names: Vec<(String, String, bool)> = Vec::new();
        if !row.first.is_empty() {
            names.push((join(&row.first, &row.last), "Taxpayer".into(), false));
        }
        if !row.spouse_first.is_empty() {
            names.push((join(&row.spouse_first, &row.last), "Spouse".into(), true));
        }
        if names.is_empty() {
            for (i, n) in names_in(name).into_iter().enumerate() {
                names.push((n, if i > 0 { "Spouse" } else { "Taxpayer" }.into(), i > 0));
            }
        }
        ClientContacts {
            client_id: format!("u{id}"),
            code: id.into(),
            name: name.into(),
            partner: None,
            planned,
            names,
        }
    }

    /// PA-106: a request is found by the contact the person confirmed, on every client that
    /// has it, each with the person's role there; by name only when no contact is on file.
    #[test]
    fn requests_are_found_by_confirmed_contact_then_by_name() {
        let dir = vec![
            dir_entry(
                "1042",
                "Sample, John & Jane",
                row(&[
                    ("first", "John"),
                    ("last", "Sample"),
                    ("email", "john@example.test"),
                    ("sfirst", "Jane"),
                    ("semail", "jane@example.test"),
                    ("scell", "555-201-0002"),
                ]),
                None,
            ),
            dir_entry(
                "1077",
                "Sample Farms LLC",
                row(&[
                    ("first", "Jane"),
                    ("last", "Sample"),
                    ("email", "Jane@Example.test"),
                ]),
                Some("llc"),
            ),
            dir_entry("1090", "Doe, Alex & Sam Roe", Row::default(), None),
        ];
        let f = find("jane@example.test", "Janie", &dir);
        assert_eq!(
            f.iter()
                .map(|x| (x.code.as_str(), x.role.as_str(), x.by, x.spouse))
                .collect::<Vec<_>>(),
            vec![
                ("1042", "Spouse", FoundBy::Email, true),
                ("1077", "Owner", FoundBy::Email, false),
            ]
        );
        let f = find("+15552010002", "", &dir);
        assert_eq!((f.len(), f[0].by), (1, FoundBy::Cell));
        // No contact on file: the name is a lead, from the client's name when no names are on
        // file ("Doe, Alex & Sam Roe" is Alex Doe and Sam Roe).
        let f = find("sam@example.test", "Sam J. Roe", &dir);
        assert_eq!(
            f.iter()
                .map(|x| (x.code.as_str(), x.role.as_str(), x.by))
                .collect::<Vec<_>>(),
            vec![("1090", "Spouse", FoundBy::Name)]
        );
        assert!(find("x@example.test", "Nobody Here", &dir).is_empty());
        // One word is not a name to go by.
        assert!(find("x@example.test", "Sample", &dir).is_empty());
    }

    /// The old Admin › Portal email and mobile columns move over to the Portal contacts once.
    #[test]
    fn the_old_record_columns_are_adopted() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(crate::clients::master_dir(root)).unwrap();
        let csv = root.join("clients.csv");
        std::fs::write(
            &csv,
            "ID,Name,E Mail,Mobile\n1001,\"Sample, John\",john@example.test; old@example.test,555-201-0001\n1002,Other,,\n",
        )
        .unwrap();
        let map = ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            partner: None,
            extras: vec!["E Mail".into(), "Mobile".into()],
        };
        let list =
            crate::clients::import_csv_tin(root, &csv, &map, &Default::default(), None).unwrap();
        crate::clients::use_csv_in_intake(root, &csv, &map).unwrap();
        assert_eq!(
            adopt_record_columns(root, Some("E_Mail"), Some("Mobile")).unwrap(),
            1
        );
        let cols = columns(root).unwrap();
        assert_eq!(cols.email.as_deref(), Some("E Mail"));
        assert_eq!(cols.cell.as_deref(), Some("Mobile"));
        let john = list.match_by_id("1001").unwrap().uuid.to_string();
        let rec = record(root, &john).unwrap();
        assert_eq!(rec.csv.email, "john@example.test");
        assert_eq!(rec.csv.cell, "555-201-0001");
        set_one_by_hand(root, &john, "spouse_email", "jane@example.test").unwrap();
        assert_eq!(
            record(root, &john).unwrap().effective().spouse_email,
            "jane@example.test"
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

    fn import(root: &Path, csv_text: &str, cols: &ContactColumns) -> ClientList {
        let csv = root.join("clients.csv");
        std::fs::write(&csv, csv_text).unwrap();
        let map = ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            partner: None,
            extras: vec![],
        };
        let list =
            crate::clients::import_csv_tin(root, &csv, &map, &Default::default(), None).unwrap();
        let table = filer_core::clients::read_csv(&csv).unwrap();
        store_from_csv(root, &list, &csv_rows(&table, &map, cols).unwrap()).unwrap();
        list
    }

    /// Contacts are kept in the cabinet at Import (the CSV is not needed after), hand edits
    /// win over a later CSV, and Clear goes back to the CSV.
    #[test]
    fn contacts_are_stored_and_hand_edits_win() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(crate::clients::master_dir(root)).unwrap();
        let cols = ContactColumns {
            email: Some("Email".into()),
            cell: Some("Cell".into()),
            ..Default::default()
        };
        let list = import(
            root,
            "ID,Name,Email,Cell\n1001,\"Sample, John\",john@example.test,\n1002,Sample Farms LLC,,555-201-0003\n1003,Nobody,,\n",
            &cols,
        );
        std::fs::remove_file(root.join("clients.csv")).unwrap();
        let (got, none) = read(root).unwrap();
        assert_eq!(none, 1);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].planned.people[0].by(), "john@example.test");
        assert_eq!(got[1].planned.people[0].by(), "text +15552010003");

        let john = list.match_by_id("1001").unwrap().uuid.to_string();
        let farms = list.match_by_id("1002").unwrap().uuid.to_string();
        let mut row = record(root, &john).unwrap().effective();
        row.email = "john.typed@example.test".into();
        row.spouse_first = "Jane".into();
        row.spouse_email = "jane@example.test".into();
        let rec = set_by_hand(root, &john, &row).unwrap();
        assert_eq!(rec.hand.len(), 3);
        // A later CSV changes the email: the typed one stays; the CSV's is recorded.
        import(
            root,
            "ID,Name,Email,Cell\n1001,\"Sample, John\",john.csv@example.test,555-201-0001\n",
            &cols,
        );
        let rec = record(root, &john).unwrap();
        assert_eq!(rec.csv.email, "john.csv@example.test");
        assert_eq!(rec.effective().email, "john.typed@example.test");
        assert_eq!(rec.effective().cell, "555-201-0001");
        assert_eq!(rec.effective().spouse_email, "jane@example.test");
        // A client the CSV left out leaves the list but keeps its contacts, should it come back.
        assert_eq!(record(root, &farms).unwrap().csv.cell, "555-201-0003");
        let rec = clear_hand(root, &john).unwrap();
        assert_eq!(rec.effective().email, "john.csv@example.test");
        assert_eq!(rec.effective().spouse_email, "");
    }

    #[test]
    fn the_preview_says_what_an_import_changes() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(crate::clients::master_dir(root)).unwrap();
        let cols = ContactColumns {
            email: Some("Email".into()),
            ..Default::default()
        };
        let list = import(
            root,
            "ID,Name,Email\n1001,\"Sample, John\",john@example.test\n1002,\"Taylor, M.\",\n1003,\"Doe, Alex\",\n",
            &cols,
        );
        let john = list.match_by_id("1001").unwrap().uuid.to_string();
        set_by_hand(
            root,
            &john,
            &Row {
                email: "typed@example.test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let csv = root.join("new.csv");
        std::fs::write(
            &csv,
            "ID,Name,Email\n1001,\"Sample, John\",john.new@example.test\n1002,\"Taylor, Morgan\",morgan@example.test\n1004,New Client,\n",
        )
        .unwrap();
        let table = filer_core::clients::read_csv(&csv).unwrap();
        let map = ColumnMap {
            id: Some("ID".into()),
            name: Some("Name".into()),
            partner: None,
            extras: vec![],
        };
        let m = load(root).unwrap();
        let p = preview(&table, &map, &cols, m.list.as_ref(), &m.meta.contacts).unwrap();
        assert_eq!(p.new, vec![("1004".to_string(), "New Client".to_string())]);
        assert_eq!(
            p.missing,
            vec![("1003".to_string(), "Doe, Alex".to_string())]
        );
        assert_eq!(
            p.renamed,
            vec![(
                "1002".to_string(),
                "Taylor, M.".to_string(),
                "Taylor, Morgan".to_string()
            )]
        );
        assert_eq!(p.contacts.len(), 2);
        assert_eq!(p.contacts[0].id, "1001");
        assert_eq!(p.contacts[0].old, "john@example.test");
        assert!(p.contacts[0].kept_by_hand);
        assert_eq!(p.contacts[1].new, "morgan@example.test");
        assert!(!p.contacts[1].kept_by_hand);
    }
}
