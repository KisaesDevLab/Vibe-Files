//! UltraTax CS print capture set up the way UltraTax itself is (PA-22, owner 2026-11-03): its
//! PDF File Options. The **file name layout** (which variable sits in which position, the K-1
//! recipient after the document name, the state postal code) gives the pattern that reads a
//! printed name; the **document names** table lists UltraTax's documents (government, client and
//! preparer copies of the return and K-1 package, organizers, extension, input screens) with the
//! custom name the office set in UltraTax, and for each: the doc type, the folder, whether it is
//! filed at all, and the name it is filed under.
//!
//! Filer files what reaches its inbox by its rules, whose file name tokens are fixed; so the
//! file name is rendered here and handed to Filer as the name's remainder, and each row gets a
//! managed Filer rule that recognizes its document name in that remainder (longest first) and
//! files it with `{Rest}.{Ext}`. A rename therefore keeps `{Document}` (or `{Printed}`, the name
//! as printed), and a K-1 row that copies to the partner ends with the recipient.

use serde::{Deserialize, Serialize};

/// The variables UltraTax can put in a PDF file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Var {
    ClientId,
    ClientName,
    EntityType,
    DocumentName,
    TaxYear,
    YearEnd,
    DatePrinted,
}

impl Var {
    pub const ALL: [Var; 7] = [
        Var::ClientId,
        Var::ClientName,
        Var::EntityType,
        Var::DocumentName,
        Var::TaxYear,
        Var::YearEnd,
        Var::DatePrinted,
    ];

    /// As UltraTax labels it.
    pub fn label(self) -> &'static str {
        match self {
            Var::ClientId => "Client ID",
            Var::ClientName => "Client Name",
            Var::EntityType => "Federal Entity Type",
            Var::DocumentName => "Document Name",
            Var::TaxYear => "Tax Year",
            Var::YearEnd => "Client Year End",
            Var::DatePrinted => "Date Printed",
        }
    }
}

/// UltraTax's PDF File Options › Options tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// Position (1-based) of each variable; 0 = not in the name.
    pub client_id: u8,
    pub client_name: u8,
    pub entity_type: u8,
    pub document_name: u8,
    pub tax_year: u8,
    #[serde(default)]
    pub year_end: u8,
    #[serde(default)]
    pub date_printed: u8,
    /// K-1 recipient name after the document name (K-1 package files only).
    #[serde(default = "yes")]
    pub k1_recipient_after_document: bool,
    /// "Print separate PDF files for each federal and state return": the state's postal code
    /// follows this variable. `None`: one file for all returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code_after: Option<Var>,
}

fn yes() -> bool {
    true
}

impl Default for Layout {
    /// The office's setting (owner's screenshot 2026-11-03): name, year, entity type, document,
    /// ID; K-1 recipient after the document name.
    fn default() -> Self {
        Layout {
            client_name: 1,
            tax_year: 2,
            entity_type: 3,
            document_name: 4,
            client_id: 5,
            year_end: 0,
            date_printed: 0,
            k1_recipient_after_document: true,
            postal_code_after: None,
        }
    }
}

impl Layout {
    pub fn position(&self, v: Var) -> u8 {
        match v {
            Var::ClientId => self.client_id,
            Var::ClientName => self.client_name,
            Var::EntityType => self.entity_type,
            Var::DocumentName => self.document_name,
            Var::TaxYear => self.tax_year,
            Var::YearEnd => self.year_end,
            Var::DatePrinted => self.date_printed,
        }
    }

    pub fn set_position(&mut self, v: Var, p: u8) {
        let slot = match v {
            Var::ClientId => &mut self.client_id,
            Var::ClientName => &mut self.client_name,
            Var::EntityType => &mut self.entity_type,
            Var::DocumentName => &mut self.document_name,
            Var::TaxYear => &mut self.tax_year,
            Var::YearEnd => &mut self.year_end,
            Var::DatePrinted => &mut self.date_printed,
        };
        *slot = p;
    }

    /// The variables in name order.
    pub fn order(&self) -> Vec<Var> {
        let mut v: Vec<Var> = Var::ALL
            .into_iter()
            .filter(|&v| self.position(v) > 0)
            .collect();
        v.sort_by_key(|&v| self.position(v));
        v
    }

    /// What is wrong with the layout, if anything.
    pub fn problem(&self) -> Option<String> {
        let order = self.order();
        for w in order.windows(2) {
            if self.position(w[0]) == self.position(w[1]) {
                return Some(format!(
                    "{} and {} are both in position {}.",
                    w[0].label(),
                    w[1].label(),
                    self.position(w[0])
                ));
            }
        }
        if self.client_id == 0 {
            return Some(
                "The client ID must be in the name: it says which client a file is for.".into(),
            );
        }
        if self.document_name == 0 {
            return Some("The document name must be in the name: it says what a file is.".into());
        }
        // The K-1 partner and the client name would run into each other (QA2 E5).
        if self.k1_recipient_after_document && self.client_name == self.document_name + 1 {
            return Some(
                "With the K-1 recipient after the document name, the client name cannot come right after the document name: the two cannot be told apart.".into(),
            );
        }
        None
    }

    /// The regex reading a printed file name (`.pdf` included). Groups: `client`, `id`, `form`,
    /// `amended`, `doc` (a K-1 name, then maybe `recipient`) or `doc_o` (any other name),
    /// `year`, `state`, `ye`, `printed`. Names of `rows` are recognized whole (longest first);
    /// other document names still read. Only a K-1 name takes a recipient after it (QA2 E4).
    pub fn regex(&self, rows: &[Row]) -> String {
        let (k1, other): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.is_k1());
        let alts = |rs: &[&Row]| {
            let mut names: Vec<String> = rs.iter().flat_map(|r| r.names()).collect();
            names.sort_by_key(|d| std::cmp::Reverse(d.len()));
            names.dedup();
            names.iter().map(|d| regex::escape(d)).collect::<Vec<_>>()
        };
        let (k1, mut other) = (alts(&k1), alts(&other));
        other.push(".+?".into());
        let mut doc = String::from("(?:(?P<amended>Amended|Superseded)_)?(?:");
        if !k1.is_empty() {
            doc.push_str(&format!("(?P<doc>{})", k1.join("|")));
            if self.k1_recipient_after_document {
                doc.push_str("(?:_(?P<recipient>[^_]+?))??");
            }
            doc.push('|');
        }
        doc.push_str(&format!("(?P<doc_o>{}))", other.join("|")));
        format!("(?i)^{}\\.pdf$", self.join(doc, true))
    }

    /// The printed name of one row, anchored to its place in the layout, with no capture groups
    /// but `who` (the K-1 partner, when `who`): what a managed Filer rule matches for `{Printed}`.
    pub fn row_body(&self, row: &Row, who: bool) -> String {
        let mut names: Vec<String> = row.names();
        names.sort_by_key(|d| std::cmp::Reverse(d.len()));
        names.dedup();
        let names: Vec<String> = names.iter().map(|d| regex::escape(d)).collect();
        let mut doc = format!("(?:(?:Amended|Superseded)_)?(?:{})", names.join("|"));
        if row.is_k1() && self.k1_recipient_after_document {
            doc.push_str(if who {
                "(?:_(?P<who>[^_]+?))??"
            } else {
                "(?:_[^_]+?)??"
            });
        }
        self.join(doc, false)
    }

    /// The layout's variables in order, `doc` standing for the document name.
    fn join(&self, doc: String, named: bool) -> String {
        let g = |name: &str, re: &str| {
            if named {
                format!("(?P<{name}>{re})")
            } else {
                format!("(?:{re})")
            }
        };
        let mut parts = Vec::new();
        for v in self.order() {
            let mut p = match v {
                Var::ClientName => g("client", ".+?"),
                Var::ClientId => g("id", "[^_ ]+"),
                Var::EntityType => g("form", "[A-Za-z0-9-]+"),
                Var::TaxYear => g("year", "(?:19|20)\\d{2}"),
                Var::YearEnd => g("ye", "[^_]+"),
                Var::DatePrinted => g("printed", "[^_]+"),
                Var::DocumentName => doc.clone(),
            };
            if self.postal_code_after == Some(v) {
                p.push_str(&format!("(?:_{})?", g("state", "[A-Z]{2}")));
            }
            parts.push(p);
        }
        parts.join("_")
    }

    /// The example UltraTax shows under the variables.
    pub fn example(&self, document: &str) -> String {
        let mut parts = Vec::new();
        for v in self.order() {
            let mut p = match v {
                Var::ClientName => "John Smith".to_string(),
                Var::ClientId => "THOMREUT".to_string(),
                Var::EntityType => "1040NR".to_string(),
                Var::TaxYear => "2025".to_string(),
                Var::YearEnd => "1225".to_string(),
                Var::DatePrinted => "11032026".to_string(),
                Var::DocumentName => {
                    let mut d = document.to_string();
                    if self.k1_recipient_after_document {
                        d.push_str("_Pat Partner*");
                    }
                    d
                }
            };
            if self.postal_code_after == Some(v) {
                p.push_str("_NE");
            }
            parts.push(p);
        }
        format!("{}.pdf", parts.join("_"))
    }
}

/// What happens to one document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Sent to Intake and filed in its folder.
    #[default]
    File,
    /// Not filed: the print is kept in the capture folder's `_processed`.
    Skip,
}

/// Where government copies are filed by default (owner 2026-11-04).
pub const GOVERNMENT_FOLDER: &str = "Income Tax/{Year}/Government Copy";
/// Where preparer copies are filed by default (owner 2026-11-04).
pub const PREPARER_FOLDER: &str = "Workpapers & Support/{Year}";

/// The doc type a K-1 row takes from the entity type (1120-S, 1065, 1041).
pub const K1_BY_FORM: &str = "K-1 (by entity type)";

/// One row of UltraTax's Document Names tab, with what Vibe Files does with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// UltraTax's default document name (`PreparerCopyTaxReturn`); also the row's identity.
    pub default_name: String,
    /// The custom name set in UltraTax (`Preparer_Copy_Tax_Return`); empty = the default.
    #[serde(default)]
    pub custom_name: String,
    /// Doc type name, or [`K1_BY_FORM`]; empty = none.
    #[serde(default)]
    pub doc_type: String,
    #[serde(default)]
    pub action: Action,
    /// Folder below the client folder; empty = the doc type's default.
    #[serde(default)]
    pub folder: String,
    /// The name it is filed under (tokens, see [`TOKENS`]); empty = as printed.
    #[serde(default)]
    pub rename: String,
    /// A K-1 for one partner is also copied to that partner's folder (when the partner is a
    /// related client or in the client list).
    #[serde(default)]
    pub copy_to_partner: bool,
}

/// UltraTax's document groups and rows, as its Document Names tab lists them: (group, label,
/// default name). The Organizer (XLS) row is left out: the capture folder takes PDFs.
pub const DOCUMENTS: [(&str, &str, &str); 13] = [
    (
        "Tax Return (government copy)",
        "GovernmentCopyTaxReturn",
        "GovernmentCopyTaxReturn",
    ),
    (
        "Tax Return (government copy)",
        "GovernmentCopyK1Package",
        "GovernmentCopyK1Package",
    ),
    (
        "Tax Return (client copy)",
        "ArchivedTaxReturn",
        "ArchivedTaxReturn",
    ),
    (
        "Tax Return (client copy)",
        "TaxReturn-ActionRequired",
        "TaxReturn-ActionRequired",
    ),
    (
        "Tax Return (client copy)",
        "ArchiveK1Package",
        "ArchiveK1Package",
    ),
    (
        "Tax Return (client copy)",
        "ActionRequiredK1Package",
        "ActionRequiredK1Package",
    ),
    (
        "Tax Return (client copy)",
        "TaxSourceDocuments",
        "TaxSourceDocuments",
    ),
    (
        "Tax Return (preparer copy)",
        "PreparerCopyTaxReturn",
        "PreparerCopyTaxReturn",
    ),
    (
        "Tax Return (preparer copy)",
        "PreparerCopyK1Package",
        "PreparerCopyK1Package",
    ),
    ("Organizer", "Organizer (PDF)", "Organizer"),
    (
        "Organizer",
        "Organizer-ActionRequired",
        "Organizer-ActionRequired",
    ),
    ("Other", "Extension", "Extension"),
    ("Other", "InputScreens", "InputScreens"),
];

/// The group a row belongs to (by its default name).
pub fn group_of(default_name: &str) -> &'static str {
    DOCUMENTS
        .iter()
        .find(|(_, _, d)| *d == default_name)
        .map_or("Other", |(g, _, _)| *g)
}

/// UltraTax's label for a row.
pub fn label_of(default_name: &str) -> &str {
    DOCUMENTS
        .iter()
        .find(|(_, _, d)| *d == default_name)
        .map_or(default_name, |(_, l, _)| *l)
}

/// The rows seeded for this office: its custom names (owner's screenshot 2026-11-03); every
/// document filed, in its doc type's default folder, under its printed name. Government and
/// preparer copies are filed like the client copy until the firm says otherwise.
pub fn default_rows() -> Vec<Row> {
    let row = |d: &str, custom: &str, doc_type: &str, copy: bool| Row {
        default_name: d.into(),
        custom_name: custom.into(),
        doc_type: doc_type.into(),
        action: Action::File,
        folder: String::new(),
        rename: String::new(),
        copy_to_partner: copy,
    };
    // Owner 2026-11-04 (QUESTIONS Q-39, Q-40): government copies in their own folder under the
    // year, preparer copies with the workpapers; only the client-copy K-1 package goes to the
    // partner.
    let gov = |mut r: Row| {
        r.folder = GOVERNMENT_FOLDER.into();
        r
    };
    let prep = |mut r: Row| {
        r.folder = PREPARER_FOLDER.into();
        r
    };
    vec![
        gov(row(
            "GovernmentCopyTaxReturn",
            "",
            "Return copy — federal",
            false,
        )),
        gov(row("GovernmentCopyK1Package", "", K1_BY_FORM, false)),
        row(
            "ArchivedTaxReturn",
            "Tax_Return",
            "Return copy — federal",
            false,
        ),
        row(
            "TaxReturn-ActionRequired",
            "Sign_Return_Mail_These_Documents",
            "8879 e-file authorization",
            false,
        ),
        row("ArchiveK1Package", "K1_Package", K1_BY_FORM, true),
        row("ActionRequiredK1Package", "K1_Letter", K1_BY_FORM, false),
        row(
            "TaxSourceDocuments",
            "Tax_Return_Documents",
            "Workpaper",
            false,
        ),
        prep(row(
            "PreparerCopyTaxReturn",
            "Preparer_Copy_Tax_Return",
            "Return copy — federal",
            false,
        )),
        prep(row(
            "PreparerCopyK1Package",
            "Preparer_Copy_K1",
            K1_BY_FORM,
            false,
        )),
        row("Organizer", "", "Organizer", false),
        row("Organizer-ActionRequired", "", "Organizer", false),
        row("Extension", "", "Extension", false),
        row("InputScreens", "", "Workpaper", false),
    ]
}

impl Row {
    /// A K-1 document (its printed name may carry the partner after it).
    pub fn is_k1(&self) -> bool {
        self.default_name.contains("K1")
    }

    /// The name UltraTax prints for it.
    pub fn printed_name(&self) -> &str {
        if self.custom_name.trim().is_empty() {
            &self.default_name
        } else {
            self.custom_name.trim()
        }
    }

    /// Both names a print may carry (custom names can be switched off in UltraTax).
    pub fn names(&self) -> Vec<String> {
        let mut v = vec![self.default_name.clone()];
        if !self.custom_name.trim().is_empty() {
            v.push(self.custom_name.trim().to_string());
        }
        v
    }

    /// The doc type for a print of entity type `form`.
    pub fn doc_type_for(&self, form: Option<&str>) -> Option<String> {
        if self.doc_type == K1_BY_FORM {
            return k1_type(form?).map(str::to_string);
        }
        (!self.doc_type.is_empty()).then(|| self.doc_type.clone())
    }

    /// The file name template in effect: the row's own, else the profile's choice: the name as
    /// printed, or the short standard name (`short`).
    pub fn template(&self, short: bool) -> String {
        if !self.rename.trim().is_empty() {
            return self.rename.trim().to_string();
        }
        if !short {
            return "{Printed}".into();
        }
        if self.is_k1() {
            SHORT_K1.into()
        } else {
            SHORT.into()
        }
    }
}

/// The doc types a [`K1_BY_FORM`] row files under.
pub const K1_TYPES: [&str; 3] = ["K-1 (1120-S)", "K-1 (1065)", "K-1 (1041)"];

/// The K-1 doc type for an entity type.
pub fn k1_type(form: &str) -> Option<&'static str> {
    let f = form.to_ascii_uppercase().replace('-', "");
    if f.starts_with("1120S") {
        Some("K-1 (1120-S)")
    } else if f.starts_with("1065") {
        Some("K-1 (1065)")
    } else if f.starts_with("1041") {
        Some("K-1 (1041)")
    } else {
        None
    }
}

/// The short standard file name (the client and ID are in the folder path already).
pub const SHORT: &str = "{Year} {Form} {State} {Amended} {Document}";
/// The short standard name of a K-1: ends with the partner, one file per partner.
pub const SHORT_K1: &str = "{Year} {Form} {State} {Amended} {Document} - {Recipient}";

/// Rename tokens, for the editor's help.
pub const TOKENS: [(&str, &str); 10] = [
    ("{Printed}", "the name as UltraTax printed it"),
    (
        "{Document}",
        "the document name as printed (Preparer_Copy_Tax_Return)",
    ),
    ("{DocType}", "the doc type"),
    ("{Client}", "the client's name in the client list"),
    ("{ID}", "the client ID"),
    ("{Year}", "the tax year"),
    ("{Form}", "the federal entity type (1040, 1120S, …)"),
    ("{Recipient}", "the K-1 partner"),
    ("{State}", "the state postal code (separate state returns)"),
    (
        "{Amended}",
        "\"Amended\" or \"Superseded\" when UltraTax added it",
    ),
];

/// Values for [`render`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values {
    pub printed: String,
    pub document: String,
    pub doc_type: String,
    pub client: String,
    pub id: String,
    pub year: String,
    pub form: String,
    pub recipient: String,
    pub state: String,
    pub amended: String,
}

/// Fills `template`; parts left empty leave no doubled separators behind.
pub fn render(template: &str, v: &Values) -> String {
    // Literal text and values apart: values go in as they are (a client "Acme Corp -", a
    // custom name with "__"), only the template's own separators are tidied (QA3 E2).
    enum Part<'a> {
        Lit(String),
        Val(&'a str),
    }
    let value = |tok: &str| -> Option<&String> {
        Some(match tok {
            "{Printed}" => &v.printed,
            "{Document}" => &v.document,
            "{DocType}" => &v.doc_type,
            "{Client}" => &v.client,
            "{ID}" => &v.id,
            "{Year}" => &v.year,
            "{Form}" => &v.form,
            "{Recipient}" => &v.recipient,
            "{State}" => &v.state,
            "{Amended}" => &v.amended,
            _ => return None,
        })
    };
    let mut parts: Vec<Part> = Vec::new();
    let mut lit = String::new();
    let mut rest = template;
    while let Some(c) = rest.chars().next() {
        if c == '{'
            && let Some(end) = rest.find('}')
            && let Some(val) = value(&rest[..=end])
        {
            if !val.is_empty() {
                parts.push(Part::Lit(std::mem::take(&mut lit)));
                parts.push(Part::Val(val));
            }
            rest = &rest[end + 1..];
            continue;
        }
        lit.push(c);
        rest = &rest[c.len_utf8()..];
    }
    parts.push(Part::Lit(lit));
    // Each run of separators in the literal text becomes one ("_", " ", or " - " kept as the
    // template meant it); none at the very start or end.
    let tidy = |l: &str, first: bool, last: bool| -> String {
        let mut out = String::new();
        let mut sep = String::new();
        let flush = |out: &mut String, sep: &mut String| {
            let t = sep.trim();
            out.push_str(&if t.is_empty() {
                if sep.contains('_') { "_" } else { " " }.to_string()
            } else if sep.starts_with(' ') || sep.ends_with(' ') {
                format!(" {} ", t.chars().next().expect("not empty"))
            } else {
                t.chars().take(1).collect()
            });
            sep.clear();
        };
        for ch in l.chars() {
            if matches!(ch, ' ' | '_' | '-') {
                sep.push(ch);
            } else {
                if !sep.is_empty() && !(first && out.is_empty()) {
                    flush(&mut out, &mut sep);
                }
                sep.clear();
                out.push(ch);
            }
        }
        if !sep.is_empty() && !last && !(first && out.is_empty()) {
            flush(&mut out, &mut sep);
        }
        out
    };
    // Literals between two values are merged where values were left out.
    let mut merged: Vec<Part> = Vec::new();
    for p in parts {
        match (merged.last_mut(), p) {
            (Some(Part::Lit(a)), Part::Lit(b)) => a.push_str(&b),
            (_, p) => merged.push(p),
        }
    }
    let n = merged.len();
    let mut out = String::new();
    for (k, p) in merged.into_iter().enumerate() {
        match p {
            Part::Val(v) => out.push_str(v),
            Part::Lit(l) => out.push_str(&tidy(&l, k == 0, k + 1 == n)),
        }
    }
    out
}

/// What is wrong with `row`'s rename, if anything.
pub fn rename_problem(row: &Row, short: bool) -> Option<String> {
    if let Some(e) = folder_problem(&row.folder) {
        return Some(format!("{}: {e}", label_of(&row.default_name)));
    }
    let t = row.template(short);
    let t = t.as_str();
    if let Some(c) = t.chars().find(|c| r#"\/:*?"<>|"#.contains(*c)) {
        return Some(format!(
            "{}: the name cannot contain {c}",
            label_of(&row.default_name)
        ));
    }
    let mut rest = t.to_string();
    for (k, _) in TOKENS {
        rest = rest.replace(k, "");
    }
    if rest.contains('{') || rest.contains('}') {
        return Some(format!(
            "{}: unknown {{token}} in the name",
            label_of(&row.default_name)
        ));
    }
    if !t.contains("{Document}") && !t.contains("{Printed}") {
        return Some(format!(
            "{}: the name must keep {{Document}} or {{Printed}}: Intake recognizes the document by it",
            label_of(&row.default_name)
        ));
    }
    if t.ends_with('.') || t.ends_with(' ') {
        return Some(format!(
            "{}: the name cannot end with a dot or a space",
            label_of(&row.default_name)
        ));
    }
    // The partner is read from the name: right after the document name, or after plain text
    // (" - "); an optional part before it would take the start of the name (QA3 E5).
    if row.copy_to_partner
        && let Some(i) = t.find("{Recipient}")
    {
        let before = t[..i].trim_end_matches([' ', '_', '-']);
        if before.ends_with('}') && !before.ends_with("{Document}") {
            return Some(format!(
                "{}: put {{Recipient}} right after {{Document}}, or after text such as \" - \"",
                label_of(&row.default_name)
            ));
        }
    }
    if row.copy_to_partner && !(t.contains("{Recipient}") || t.contains("{Printed}")) {
        return Some(format!(
            "{}: to copy to the partner the name must keep {{Recipient}} (or {{Printed}})",
            label_of(&row.default_name)
        ));
    }
    None
}

/// What is wrong with a folder typed for a row (below the client folder), if anything.
pub fn folder_problem(folder: &str) -> Option<String> {
    if let Some(c) = folder.chars().find(|c| r#"\:*?"<>|"#.contains(*c)) {
        return Some(format!(
            "the folder cannot contain {c} (use / between folders)"
        ));
    }
    let mut rest = folder.to_string();
    for k in ["{Client}", "{ID}", "{Year}"] {
        rest = rest.replace(k, "");
    }
    if rest.contains('{') || rest.contains('}') {
        return Some("a folder takes only {Client}, {ID} and {Year}".into());
    }
    let segs: Vec<&str> = folder.trim().trim_matches('/').split('/').collect();
    if folder.trim().is_empty() {
        return None;
    }
    for seg in segs {
        if seg.trim().is_empty() {
            return Some("a folder name is empty (two / in a row)".into());
        }
        if seg.trim() == "." || seg.trim() == ".." {
            return Some("a folder cannot be . or ..".into());
        }
        if seg.ends_with(' ') || seg.ends_with('.') {
            return Some(format!(
                "\"{seg}\": Windows does not allow a folder name ending in a space or a dot"
            ));
        }
        let base = seg
            .split('.')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
            || ((base.starts_with("COM") || base.starts_with("LPT"))
                && base.len() == 4
                && base.as_bytes()[3].is_ascii_digit());
        if reserved {
            return Some(format!("\"{seg}\" is a name Windows reserves"));
        }
    }
    None
}

/// The Filer rule pattern that recognizes the names Vibe Files renders for `row`: the row's file
/// name template with each token replaced by what it can render to, anchored at both ends
/// (QA2 E1). So a document name inside a client's name, or an ordinary file someone drops in
/// the inbox, is never taken by the row. The K-1 partner is captured as `who` (copies).
/// `layout` gives the shape of `{Printed}`; a leading year may be added by the pipeline when
/// the name would otherwise give Filer the wrong year (QA2 E2).
pub fn rule_pattern(row: &Row, layout: &Layout, short: bool) -> String {
    let t = row.template(short);
    let mut names: Vec<String> = row.names();
    names.sort_by_key(|d| std::cmp::Reverse(d.len()));
    names.dedup();
    let doc = format!(
        "(?:{})",
        names
            .iter()
            .map(|d| regex::escape(d))
            .collect::<Vec<_>>()
            .join("|")
    );
    let mut who = row.copy_to_partner;
    let mut out = String::from(r"^(?:(?:19|20)\d{2} )?");
    let mut rest = t.as_str();
    while !rest.is_empty() {
        if rest.starts_with('{')
            && let Some(end) = rest.find('}')
        {
            let tok = &rest[..=end];
            rest = &rest[end + 1..];
            out.push_str(&match tok {
                "{Printed}" => {
                    let b = format!("(?:{})", layout.row_body(row, who));
                    if row.is_k1() && layout.k1_recipient_after_document {
                        who = false;
                    }
                    b
                }
                "{Document}" => doc.clone(),
                "{Amended}" => "(?:Amended|Superseded)?".into(),
                "{Year}" => r"(?:(?:19|20)\d{2})?".into(),
                "{Form}" => "[A-Za-z0-9-]*".into(),
                "{State}" => "(?:[A-Z]{2})?".into(),
                "{ID}" => r"[^\s_]*".into(),
                "{Recipient}" if who => {
                    who = false;
                    "(?P<who>.*?)".into()
                }
                _ => ".*?".into(),
            });
            continue;
        }
        let ch = rest.chars().next().expect("not empty");
        rest = &rest[ch.len_utf8()..];
        if matches!(ch, ' ' | '_' | '-') {
            if !out.ends_with(r"[\s_-]*") {
                out.push_str(r"[\s_-]*");
            }
        } else {
            out.push_str(&regex::escape(&ch.to_string()));
        }
    }
    // The inbox names a second print of the same name `… (2).pdf` (QA3 e2e #1).
    out.push_str(r"(?: \(\d+\))?$");
    out
}

/// How long a row's phrase is (the order rules are tried in).
pub fn phrase_len(row: &Row) -> usize {
    row.names()
        .iter()
        .map(|n| crate::pipeline::words(n).len())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_layout_reads_the_office_names() {
        let l = Layout::default();
        let re = regex::Regex::new(&l.regex(&default_rows())).unwrap();
        let c = re
            .captures("Harbor Point Farms LLC_2025_1120S_Preparer_Copy_K1_Dana Reyes_HARB1234.pdf")
            .unwrap();
        assert_eq!(&c["client"], "Harbor Point Farms LLC");
        assert_eq!(&c["year"], "2025");
        assert_eq!(&c["form"], "1120S");
        assert_eq!(&c["doc"], "Preparer_Copy_K1");
        assert_eq!(&c["recipient"], "Dana Reyes");
        assert_eq!(&c["id"], "HARB1234");
        let c = re
            .captures("Reyes, Dana & Lee_2025_1040_Amended_Tax_Return_REYE5521.pdf")
            .unwrap();
        assert_eq!(&c["doc_o"], "Tax_Return");
        assert_eq!(&c["amended"], "Amended");
        assert!(c.name("recipient").is_none());
        // A document not in the table still reads, whole: only K-1 names take a recipient
        // (QA2 E4).
        let c = re
            .captures("Reyes, Dana & Lee_2025_1040_Invoice_REYE5521.pdf")
            .unwrap();
        assert_eq!(&c["doc_o"], "Invoice");
        let c = re
            .captures("Reyes, Dana & Lee_2025_1040_Form_8879_REYE5521.pdf")
            .unwrap();
        assert_eq!(&c["doc_o"], "Form_8879");
        assert!(c.name("recipient").is_none());
        assert_eq!(
            l.example("Tax_Return"),
            "John Smith_2025_1040NR_Tax_Return_Pat Partner*_THOMREUT.pdf"
        );
    }

    /// QA2 E1/E6/E7: a row's rule matches only the names rendered for it, anchored; never a
    /// document name inside a client's name or an ordinary inbox file.
    #[test]
    fn rule_patterns_are_anchored_to_the_rendered_shape() {
        let l = Layout::default();
        let rows = default_rows();
        let get = |d: &str| rows.iter().find(|r| r.default_name == d).unwrap();
        let re = |d: &str, short: bool| {
            regex::RegexBuilder::new(&rule_pattern(get(d), &l, short))
                .case_insensitive(true)
                .build()
                .unwrap()
        };
        let tax = re("ArchivedTaxReturn", false);
        assert!(tax.is_match("Reyes_2025_1040_Tax_Return_REYE5521"));
        assert!(tax.is_match("Reyes_2025_1040_Amended_Tax_Return_REYE5521"));
        // A second print of the same name, renamed by the inbox (QA3 e2e #1).
        assert!(tax.is_match("Reyes_2025_1040_Tax_Return_REYE5521 (2)"));
        assert!(tax.is_match("2019 Smith 2019 Trust_2025_1041_Tax_Return_SMIT1234"));
        for other in [
            "Tax Return Pros LLC_2025_1120S_InputScreens_TAXR1234",
            "Tax Return Pros LLC_2025_1120S_K1_Package_Ann Lee_TAXR1234",
            "Reyes_2025_1040_Preparer_Copy_Tax_Return_REYE5521",
            "Reyes_2025_1040_Tax_Return_Documents_REYE5521",
            "2025 Tax Return signed",
        ] {
            assert!(!tax.is_match(other), "{other}");
        }
        assert!(
            !re("InputScreens", false).is_match("Input Screens Inc_2025_1120_Extension_INPU1234")
        );
        let k1 = re("ArchiveK1Package", false);
        let c = k1
            .captures("Harbor Point Farms LLC_2025_1120S_K1_Package_Dana Reyes_HARB1234")
            .unwrap();
        assert_eq!(&c["who"], "Dana Reyes");
        let c = k1.captures("Harbor_2025_1065_K1_Package_HARB1234").unwrap();
        assert!(c.name("who").is_none());
        let short = re("ArchiveK1Package", true);
        let c = short.captures("2025 1065 K1_Package - Dana Reyes").unwrap();
        assert_eq!(&c["who"], "Dana Reyes");
        assert!(re("ArchivedTaxReturn", true).is_match("2025 1040 Tax_Return"));
        assert!(re("ArchivedTaxReturn", true).is_match("2025 1040 NE Tax_Return"));
        assert!(!re("ArchivedTaxReturn", true).is_match("2025 1040 Tax_Return_Documents"));
        // A rename with the partner joined by "_" still captures the partner.
        let mut r = get("ArchiveK1Package").clone();
        r.rename = "{Year} {Document}_{Recipient}".into();
        let p = regex::RegexBuilder::new(&rule_pattern(&r, &l, false))
            .case_insensitive(true)
            .build()
            .unwrap();
        assert_eq!(&p.captures("2025 K1_Package_Ryan").unwrap()["who"], "Ryan");
        // The client name right after the document name cannot be read with a recipient.
        let bad = Layout {
            client_name: 5,
            client_id: 1,
            ..Default::default()
        };
        assert!(bad.problem().is_some());
        assert!(folder_problem("Income Tax//{Year}").is_some());
        assert!(folder_problem("Income Tax/CON").is_some());
        assert!(folder_problem("Income Tax/x. ").is_some());
        assert!(folder_problem("Income Tax/{Year}/Government Copy").is_none());
    }

    #[test]
    fn render_tidies_empty_parts() {
        let v = Values {
            year: "2025".into(),
            document: "Tax_Return".into(),
            client: "Reyes, Dana".into(),
            ..Default::default()
        };
        assert_eq!(
            render("{Year} {Document} - {Recipient}", &v),
            "2025 Tax_Return"
        );
        assert_eq!(
            render("{Year} {Amended} {Document} - {Client}", &v),
            "2025 Tax_Return - Reyes, Dana"
        );
        // Values go in as they are; only the template's own separators are tidied (QA3 E2).
        let acme = Values {
            printed: "Acme Corp -_2025_1120_Tax_Return_ACME1111".into(),
            document: "Odd__Name".into(),
            recipient: "Mary  Smith".into(),
            ..Default::default()
        };
        assert_eq!(
            render("{Printed}", &acme),
            "Acme Corp -_2025_1120_Tax_Return_ACME1111"
        );
        assert_eq!(
            render("{Document} - {Recipient}", &acme),
            "Odd__Name - Mary  Smith"
        );
        assert_eq!(render("{Year}_{Document}", &acme), "Odd__Name");
        let mut k = default_rows()
            .into_iter()
            .find(|r| r.default_name == "ArchiveK1Package")
            .unwrap();
        k.rename = "{Year} {Form} {Document} {State} {Recipient}".into();
        assert!(
            rename_problem(&k, false).is_some(),
            "optional part before the partner"
        );
        k.rename = "{Year} {Document} - {Recipient}".into();
        assert!(rename_problem(&k, false).is_none());
        k.rename = "{Document}.".into();
        assert!(rename_problem(&k, false).is_some(), "trailing dot");
        let mut r = default_rows()[0].clone();
        r.rename = "{Year} return".into();
        assert!(rename_problem(&r, false).is_some());
        r.rename = "{Year} {Document}".into();
        assert!(rename_problem(&r, false).is_none());
        // The short names keep the document name; a K-1's ends with the partner.
        for row in default_rows() {
            assert!(rename_problem(&row, true).is_none(), "{}", row.default_name);
            assert!(
                rename_problem(&row, false).is_none(),
                "{}",
                row.default_name
            );
        }
        r.folder = "Income Tax/{Year}/../x".into();
        assert!(rename_problem(&r, false).is_some());
        r.folder = "Income Tax/{Year}/Government Copy".into();
        assert!(rename_problem(&r, false).is_none());
    }
}
