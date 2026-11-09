//! Filename parsing (`ClientName_ID_rest.ext`) and tax-year detection.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Default for `Settings::id_pattern`: the ID part of `ClientName_ID_rest.ext`. Digits and capital
/// letters (UltraTax client IDs such as `THOMREUT`, owner 2026-11-02); case-sensitive, so a name
/// word like `John` is not taken for an ID.
pub const DEFAULT_ID_PATTERN: &str = r"[A-Z0-9]{4,}";
/// Years accepted in a filename: this many years back from the current year...
pub const YEARS_BACK: i32 = 10; // owner #41
/// ...through this many years ahead.
pub const YEARS_AHEAD: i32 = 1;

/// `^(?P<name>.+?)_(?P<id>ID)_(?P<rest>.+)\.(?P<ext>[^.]+)$` with the configured ID pattern.
pub fn filename_regex(id_pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(&format!(
        r"^(?P<name>.+?)_(?P<id>{id_pattern})_(?P<rest>.+)\.(?P<ext>[^.]+)$"
    ))
}

/// Checks a user-entered ID pattern. Returns a message for the Setup screen on failure.
pub fn validate_id_pattern(id_pattern: &str) -> Result<(), String> {
    if id_pattern.trim().is_empty() {
        return Err("ID pattern is empty.".into());
    }
    let whole = Regex::new(&format!("^(?:{id_pattern})$"))
        .map_err(|e| format!("ID pattern is not a valid regex: {e}"))?;
    if whole.is_match("") {
        return Err("ID pattern must not match an empty ID.".into());
    }
    if whole.capture_names().flatten().next().is_some() {
        return Err("ID pattern must not contain named groups like (?P<x>...).".into());
    }
    filename_regex(id_pattern).map_err(|e| format!("ID pattern breaks the filename regex: {e}"))?;
    Ok(())
}

/// Where UltraTax puts the K-1 recipient's name (UltraTax's "K1 Recipient Name" setting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum K1Position {
    None,
    #[default]
    AfterDocument,
}

/// The parts of an UltraTax PDF name and their positions (1 = first, `_` between parts), as set
/// in UltraTax's "File Name Variables". `None`: not in the name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct UltraTaxLayout {
    /// Read names in this layout (tried before `Name_ID_rest`).
    pub enabled: bool,
    pub client_id: Option<u8>,
    pub client_name: Option<u8>,
    pub entity_type: Option<u8>,
    pub document: Option<u8>,
    pub tax_year: Option<u8>,
    pub year_end: Option<u8>,
    pub date_printed: Option<u8>,
    pub k1: K1Position,
}

impl Default for UltraTaxLayout {
    /// UltraTax's default: `Name_Year_EntityType_Document[_K1Recipient]_ID`.
    fn default() -> Self {
        Self {
            enabled: true,
            client_id: Some(5),
            client_name: Some(1),
            entity_type: Some(3),
            document: Some(4),
            tax_year: Some(2),
            year_end: None,
            date_printed: None,
            k1: K1Position::AfterDocument,
        }
    }
}

/// One part of an UltraTax name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtField {
    ClientId,
    ClientName,
    EntityType,
    Document,
    TaxYear,
    YearEnd,
    DatePrinted,
}

impl UtField {
    pub const ALL: [UtField; 7] = [
        UtField::ClientId,
        UtField::ClientName,
        UtField::EntityType,
        UtField::Document,
        UtField::TaxYear,
        UtField::YearEnd,
        UtField::DatePrinted,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UtField::ClientId => "Client ID",
            UtField::ClientName => "Client Name",
            UtField::EntityType => "Federal Entity Type",
            UtField::Document => "Document Name",
            UtField::TaxYear => "Tax Year",
            UtField::YearEnd => "Client Year End",
            UtField::DatePrinted => "Date Printed",
        }
    }

    /// A made-up value for the Setup example.
    pub fn example(self) -> &'static str {
        match self {
            UtField::ClientId => "THOMREUT",
            UtField::ClientName => "John Smith",
            UtField::EntityType => "1040NR",
            UtField::Document => "Tax_Return",
            UtField::TaxYear => "2025",
            UtField::YearEnd => "1231",
            UtField::DatePrinted => "03152026",
        }
    }
}

impl UltraTaxLayout {
    pub fn position(&self, f: UtField) -> Option<u8> {
        match f {
            UtField::ClientId => self.client_id,
            UtField::ClientName => self.client_name,
            UtField::EntityType => self.entity_type,
            UtField::Document => self.document,
            UtField::TaxYear => self.tax_year,
            UtField::YearEnd => self.year_end,
            UtField::DatePrinted => self.date_printed,
        }
    }

    pub fn position_mut(&mut self, f: UtField) -> &mut Option<u8> {
        match f {
            UtField::ClientId => &mut self.client_id,
            UtField::ClientName => &mut self.client_name,
            UtField::EntityType => &mut self.entity_type,
            UtField::Document => &mut self.document,
            UtField::TaxYear => &mut self.tax_year,
            UtField::YearEnd => &mut self.year_end,
            UtField::DatePrinted => &mut self.date_printed,
        }
    }

    /// The fields in the name, in order.
    fn ordered(&self) -> Vec<UtField> {
        let mut v: Vec<(u8, UtField)> = UtField::ALL
            .iter()
            .filter_map(|&f| self.position(f).map(|p| (p, f)))
            .collect();
        v.sort_by_key(|(p, _)| *p);
        v.into_iter().map(|(_, f)| f).collect()
    }

    /// Problems for the Setup screen (only checked while the layout is on).
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.client_id.is_none() || self.document.is_none() {
            return Err("UltraTax file names: Client ID and Document Name need a position.".into());
        }
        let mut used: Vec<u8> = UtField::ALL
            .iter()
            .filter_map(|&f| self.position(f))
            .collect();
        used.sort_unstable();
        if used.windows(2).any(|w| w[0] == w[1]) {
            return Err("UltraTax file names: two parts have the same position.".into());
        }
        if used
            .iter()
            .enumerate()
            .any(|(i, &p)| usize::from(p) != i + 1)
        {
            return Err(format!(
                "UltraTax file names: positions must run 1, 2, 3… without gaps (now {}).",
                used.iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(())
    }

    /// The Setup example, like UltraTax's "File Name Example".
    pub fn example(&self) -> String {
        let mut parts = Vec::new();
        for f in self.ordered() {
            parts.push(f.example().to_string());
            if f == UtField::Document && self.k1 == K1Position::AfterDocument {
                parts.push("John Smith*".into());
            }
        }
        format!("{}.PDF", parts.join("_"))
    }
}

impl K1Position {
    pub const ALL: [K1Position; 2] = [K1Position::None, K1Position::AfterDocument];

    pub fn label(self) -> &'static str {
        match self {
            K1Position::None => "None",
            K1Position::AfterDocument => "After Document Name",
        }
    }
}

/// The UltraTax parts of a file name (all text as written, `_` in the document name read as
/// spaces).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UltraTaxParts {
    pub client_id: String,
    pub client_name: String,
    pub entity_type: String,
    pub document: String,
    pub k1_recipient: String,
    pub tax_year: String,
    pub year_end: String,
    pub date_printed: String,
}

/// Splits `stem` (no extension) by `layout`: the parts before Document from the front, the parts
/// after it from the back, Document (and a K-1 recipient) in between. `None` when the name has
/// too few parts, a fixed part is empty, or the Tax Year part is not a year in the window.
/// `is_recipient` tells whether the last part of the middle names a K-1 recipient; a trailing
/// `*` always does.
pub fn split_ultratax(
    stem: &str,
    layout: &UltraTaxLayout,
    current_year: i32,
    is_recipient: impl Fn(&str) -> bool,
) -> Option<UltraTaxParts> {
    if !layout.enabled || layout.validate().is_err() {
        return None;
    }
    let order = layout.ordered();
    let doc_at = order.iter().position(|f| *f == UtField::Document)?;
    let segs: Vec<&str> = stem.split('_').collect();
    let (before, after) = (doc_at, order.len() - doc_at - 1);
    if segs.len() < before + after + 1 {
        return None;
    }
    let mut parts = UltraTaxParts::default();
    let fixed = order[..doc_at]
        .iter()
        .zip(&segs[..before])
        .chain(order[doc_at + 1..].iter().zip(&segs[segs.len() - after..]));
    for (f, seg) in fixed {
        let seg = seg.trim();
        if seg.is_empty() {
            return None;
        }
        let slot = match f {
            UtField::ClientId => &mut parts.client_id,
            UtField::ClientName => &mut parts.client_name,
            UtField::EntityType => &mut parts.entity_type,
            UtField::TaxYear => &mut parts.tax_year,
            UtField::YearEnd => &mut parts.year_end,
            UtField::DatePrinted => &mut parts.date_printed,
            UtField::Document => unreachable!("document is the middle"),
        };
        *slot = seg.to_string();
    }
    if layout.tax_year.is_some()
        && !(parts.tax_year.len() == 4 && detect_year(&parts.tax_year, current_year).is_some())
    {
        return None;
    }
    let mut middle: Vec<&str> = segs[before..segs.len() - after].to_vec();
    if layout.k1 == K1Position::AfterDocument && middle.len() >= 2 {
        let last = middle[middle.len() - 1].trim();
        // UltraTax's per-recipient K-1 packages: whatever follows "...Package" is the recipient.
        let after_package = middle[middle.len() - 2]
            .trim()
            .to_ascii_lowercase()
            .ends_with("package");
        // UltraTax adds the recipient only to K-1 PDFs: a client's name after any other document
        // ("Tax_Return_Reuter Co") stays part of the document.
        let k1_doc = middle[..middle.len() - 1].iter().any(|s| {
            s.to_ascii_lowercase()
                .replace('-', "")
                .split_whitespace()
                .any(|w| w == "k1" || w == "k1s")
        });
        if !last.is_empty() && (after_package || (k1_doc && is_recipient(last))) {
            parts.k1_recipient = last.to_string();
            middle.pop();
        }
    }
    parts.document = middle
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if parts.document.is_empty() {
        return None;
    }
    Some(parts)
}

/// Reads `file_name` (no directory): as an UltraTax name when it fits `layout` and its ID is a
/// client (`is_client_id`), else as `Name_ID_rest` when that ID is a client, else as UltraTax
/// when the name fits that layout at all, else as `Name_ID_rest` or the bare name.
pub fn read_name(
    file_name: &str,
    re: Option<&Regex>,
    layout: &UltraTaxLayout,
    current_year: i32,
    is_client_id: impl Fn(&str) -> bool,
    is_recipient: impl Fn(&str) -> bool,
) -> Parsed {
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e),
        _ => (file_name, ""),
    };
    let ut = split_ultratax(stem, layout, current_year, is_recipient);
    let standard = || match re {
        Some(re) => parse_filename(file_name, re, current_year),
        None => unparsed(file_name, current_year),
    };
    let from_ut = |u: UltraTaxParts| {
        let mut rest = format!("{} {}", u.entity_type, u.document);
        if !u.k1_recipient.is_empty() {
            rest = format!("{rest} {}", u.k1_recipient);
        }
        let rest = rest.trim().to_string();
        Parsed {
            name: u.client_name.clone(),
            id: Some(u.client_id.clone()),
            year: detect_year(&u.tax_year, current_year)
                .or_else(|| detect_year(&rest, current_year)),
            rest,
            ext: ext.to_string(),
            stem: stem.to_string(),
            ultratax: Some(u),
        }
    };
    let read = match ut {
        Some(u) if is_client_id(&u.client_id) => return from_ut(u),
        Some(u) => {
            let s = standard();
            if s.id.as_deref().is_some_and(&is_client_id) {
                return s;
            }
            from_ut(u)
        }
        None => standard(),
    };
    if read.id.as_deref().is_some_and(&is_client_id) {
        return read;
    }
    // Neither reading names a client (a year or a form number in front of the ID,
    // "Name_2025_1040_Tax_Return_3280_TIAM"): a client's ID in the last 14 characters of the
    // name, nearest the end first, then in the first 14 (owner 2026-11-09).
    let Some((start, end, id)) = id_near_ends(stem, &is_client_id) else {
        return read;
    };
    // The name is the first `_` part (else what is before the ID); the rest is everything else.
    let name_end = stem.find('_').filter(|&i| i < start).unwrap_or(start);
    let sep = |c: char| c == '_' || c == '-' || c.is_whitespace();
    let rest = format!("{}_{}", &stem[name_end..start], &stem[end..])
        .split('_')
        .map(|p| p.trim_matches(sep))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let name = stem[..name_end].trim_matches(sep).to_string();
    Parsed {
        name: if name.is_empty() {
            stem.to_string()
        } else {
            name
        },
        id: Some(id),
        year: detect_year(&rest, current_year).or_else(|| detect_year(stem, current_year)),
        rest,
        ext: ext.to_string(),
        stem: stem.to_string(),
        ultratax: None,
    }
}

/// Characters at each end of a name searched for a client ID ([`read_name`]).
const ID_SEARCH_CHARS: usize = 14;

/// A client's ID near an end of `stem`: its byte span and the ID as `is_client_id` accepted it.
/// Words are runs of letters and digits. Looked for in the words ending in the last
/// [`ID_SEARCH_CHARS`] characters (from the end), then the words starting in the first
/// [`ID_SEARCH_CHARS`] (from the start). At each word, a two-part ID first (owner 2026-11-09:
/// `@@@@_@@@@` or `@@@@@@@@`): the word with its neighbour as written ("3280_TIAM") and run
/// together ("3280TIAM"), then the word alone.
fn id_near_ends(stem: &str, is_client_id: impl Fn(&str) -> bool) -> Option<(usize, usize, String)> {
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for (i, c) in stem.char_indices() {
        match (c.is_alphanumeric(), open) {
            (true, None) => open = Some(i),
            (false, Some(s)) => {
                words.push((s, i));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(s) = open {
        words.push((s, stem.len()));
    }
    let chars = stem.chars().count();
    // Byte offset of the n-th character.
    let byte_at = |n: usize| stem.char_indices().nth(n).map_or(stem.len(), |(i, _)| i);
    let tail = byte_at(chars.saturating_sub(ID_SEARCH_CHARS));
    let head = byte_at(ID_SEARCH_CHARS);
    // The pair (a, b) as written and run together, then `single` alone.
    let try_at = |pair: Option<((usize, usize), (usize, usize))>, single: (usize, usize)| {
        if let Some(((s, first_end), (bs, e))) = pair {
            let (first, second) = (&stem[s..first_end], &stem[bs..e]);
            for id in [stem[s..e].to_string(), format!("{first}{second}")] {
                if is_client_id(&id) {
                    return Some((s, e, id));
                }
            }
        }
        let id = &stem[single.0..single.1];
        is_client_id(id).then(|| (single.0, single.1, id.to_string()))
    };
    for i in (0..words.len()).rev() {
        if words[i].1 <= tail {
            break;
        }
        let pair = (i > 0).then(|| (words[i - 1], words[i]));
        if let Some(hit) = try_at(pair, words[i]) {
            return Some(hit);
        }
    }
    for i in 0..words.len() {
        if words[i].0 >= head {
            break;
        }
        let pair = words.get(i + 1).map(|&next| (words[i], next));
        if let Some(hit) = try_at(pair, words[i]) {
            return Some(hit);
        }
    }
    None
}

/// A filename split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Client name part, or the whole stem when the name doesn't follow the convention.
    pub name: String,
    /// `None` when the name doesn't follow the convention.
    pub id: Option<String>,
    /// What rules match against. For names that don't follow the convention, the whole stem.
    pub rest: String,
    /// Extension without the dot (may be empty).
    pub ext: String,
    /// First year in `rest` within the accepted window.
    pub year: Option<u16>,
    /// The whole file name without its extension (`{Original}`, V11).
    pub stem: String,
    /// The name was read with the UltraTax layout: its parts.
    pub ultratax: Option<UltraTaxParts>,
}

/// Parses `file_name` (no directory) with the filename regex.
pub fn parse_filename(file_name: &str, re: &Regex, current_year: i32) -> Parsed {
    if let Some(c) = re.captures(file_name) {
        let rest = c["rest"].to_string();
        return Parsed {
            name: c["name"].trim().to_string(),
            id: Some(c["id"].to_string()),
            year: detect_year(&rest, current_year),
            rest,
            ext: c["ext"].to_string(),
            stem: file_name
                .strip_suffix(&format!(".{}", &c["ext"]))
                .unwrap_or(file_name)
                .to_string(),
            ultratax: None,
        };
    }
    unparsed(file_name, current_year)
}

/// A name that doesn't follow the convention: the whole stem is the name and the rest.
fn unparsed(file_name: &str, current_year: i32) -> Parsed {
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e),
        _ => (file_name, ""),
    };
    Parsed {
        name: stem.to_string(),
        id: None,
        rest: stem.to_string(),
        ext: ext.to_string(),
        year: detect_year(stem, current_year),
        stem: stem.to_string(),
        ultratax: None,
    }
}

/// First run of exactly four digits in `text` that is a year in
/// `[current_year - YEARS_BACK, current_year + YEARS_AHEAD]`. Digits touching other digits
/// (like `20251`) don't count; letters and separators around them do not matter.
pub fn detect_year(text: &str, current_year: i32) -> Option<u16> {
    let range = (current_year - YEARS_BACK)..=(current_year + YEARS_AHEAD);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i - start == 4 {
            let year: i32 = text[start..i].parse().expect("four ASCII digits");
            if range.contains(&year) {
                return Some(year as u16);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pattern_parses_convention() {
        let re = filename_regex(DEFAULT_ID_PATTERN).unwrap();
        let c = re.captures("Smith John_1234_2025 1040.pdf").unwrap();
        assert_eq!(&c["name"], "Smith John");
        assert_eq!(&c["id"], "1234");
        assert_eq!(&c["rest"], "2025 1040");
        assert_eq!(&c["ext"], "pdf");
        // UltraTax-style letter IDs; a capitalised name word is not an ID.
        let c = re.captures("John Smith_THOMREUT_2025 1040.pdf").unwrap();
        assert_eq!(&c["id"], "THOMREUT");
        let c = re.captures("Smith_John_1001_notes.pdf").unwrap();
        assert_eq!((&c["name"], &c["id"]), ("Smith_John", "1001"));
    }

    fn ut(stem: &str, layout: &UltraTaxLayout) -> Option<UltraTaxParts> {
        split_ultratax(stem, layout, 2026, |who| who == "Jane Doe")
    }

    #[test]
    fn ultratax_default_layout() {
        let l = UltraTaxLayout::default();
        assert_eq!(
            l.example(),
            "John Smith_2025_1040NR_Tax_Return_John Smith*_THOMREUT.PDF"
        );
        let p = ut("John Smith_2025_1040NR_Tax_Return_THOMREUT", &l).unwrap();
        assert_eq!(p.client_name, "John Smith");
        assert_eq!(p.tax_year, "2025");
        assert_eq!(p.entity_type, "1040NR");
        assert_eq!(p.document, "Tax Return");
        assert_eq!(p.k1_recipient, "");
        assert_eq!(p.client_id, "THOMREUT");
        // After "...Package" the last part is always the recipient, client or not.
        let p = ut("Acme_2025_1065_K1_Package_Nobody Known_ACME01", &l).unwrap();
        assert_eq!(
            (p.document.as_str(), p.k1_recipient.as_str()),
            ("K1 Package", "Nobody Known")
        );
        // A client's name after a non-K-1 document stays part of the document.
        let p = ut("Acme_2025_1040_Tax_Return_Jane Doe_ACME01", &l).unwrap();
        assert_eq!(
            (p.document.as_str(), p.k1_recipient.as_str()),
            ("Tax Return Jane Doe", "")
        );
        // No * and not a client's name: part of the document.
        let p = ut("Acme_2025_1065_K1_Package_ACME01", &l).unwrap();
        assert_eq!(
            (p.document.as_str(), p.k1_recipient.as_str()),
            ("K1 Package", "")
        );
        let p = ut("Acme_2025_1065_K1_Jane Doe_ACME01", &l).unwrap();
        assert_eq!(
            (p.document.as_str(), p.k1_recipient.as_str()),
            ("K1", "Jane Doe")
        );
        // K-1 recipient off: everything in the middle is the document.
        let off = UltraTaxLayout {
            k1: K1Position::None,
            ..l.clone()
        };
        assert_eq!(
            ut("Acme_2025_1065_K1_Jane Doe_ACME01", &off)
                .unwrap()
                .document,
            "K1 Jane Doe"
        );
        // Too few parts, a non-year in the year position, a year outside the window, an empty part.
        assert!(ut("Acme_2025_1065_ACME01", &l).is_none());
        assert!(ut("Smith, John_1001_2025 1040_x_y", &l).is_none());
        assert!(ut("Acme_1999_1065_Return_ACME01", &l).is_none());
        assert!(ut("Acme_2025__Return_ACME01", &l).is_none());
        // Off: never.
        let disabled = UltraTaxLayout {
            enabled: false,
            ..l
        };
        assert!(ut("Acme_2025_1065_Return_ACME01", &disabled).is_none());
    }

    #[test]
    fn ultratax_custom_layout_and_validation() {
        let l = UltraTaxLayout {
            client_id: Some(1),
            client_name: None,
            entity_type: None,
            document: Some(3),
            tax_year: Some(2),
            year_end: None,
            date_printed: Some(4),
            ..UltraTaxLayout::default()
        };
        assert!(l.validate().is_ok());
        assert_eq!(
            l.example(),
            "THOMREUT_2025_Tax_Return_John Smith*_03152026.PDF"
        );
        let p = ut("ACME01_2025_Engagement_Letter_03152026", &l).unwrap();
        assert_eq!(p.client_id, "ACME01");
        assert_eq!(p.document, "Engagement Letter");
        assert_eq!(p.date_printed, "03152026");

        let dup = UltraTaxLayout {
            client_name: Some(5),
            ..UltraTaxLayout::default()
        };
        assert!(dup.validate().unwrap_err().contains("same position"));
        let gap = UltraTaxLayout {
            client_id: Some(7),
            ..UltraTaxLayout::default()
        };
        assert!(gap.validate().unwrap_err().contains("without gaps"));
        let no_id = UltraTaxLayout {
            client_id: None,
            ..UltraTaxLayout::default()
        };
        assert!(no_id.validate().is_err());
    }

    #[test]
    fn read_name_prefers_the_layout_whose_id_is_a_client() {
        let re = filename_regex(DEFAULT_ID_PATTERN).unwrap();
        let l = UltraTaxLayout::default();
        let read = |name: &str, ids: &[&str]| {
            read_name(name, Some(&re), &l, 2026, |id| ids.contains(&id), |_| false)
        };
        let p = read(
            "John Smith_2025_1040_Tax_Return_THOMREUT.pdf",
            &["THOMREUT"],
        );
        assert_eq!(p.id.as_deref(), Some("THOMREUT"));
        assert_eq!(p.name, "John Smith");
        assert_eq!(p.rest, "1040 Tax Return");
        assert_eq!(p.year, Some(2025));
        assert_eq!(p.ext, "pdf");
        // Fits both; only the standard ID (2025) is a client.
        let p = read("John Smith_2025_1040_Tax_Return_THOMREUT.pdf", &["2025"]);
        assert!(p.ultratax.is_none());
        assert_eq!(p.id.as_deref(), Some("2025"));
        // Neither ID known: UltraTax (the standard reading would take the year as the ID).
        let p = read("John Smith_2025_1040_Tax_Return_THOMREUT.pdf", &[]);
        assert!(p.ultratax.is_some());
        // A standard name doesn't fit the layout.
        let p = read("Smith, John_1001_2025 1040.pdf", &[]);
        assert!(p.ultratax.is_none());
        assert_eq!(p.id.as_deref(), Some("1001"));
        // A year and a form before the ID, the layout off (as in Vibe Files): the part that is
        // a client's ID, not the year (owner 2026-11-09).
        let off = UltraTaxLayout {
            enabled: false,
            ..UltraTaxLayout::default()
        };
        let name = "Tiam, Hrang & Tial Cin_2025_1040_Tax_Return_3280_TIAM.pdf";
        let p = read_name(name, Some(&re), &off, 2026, |id| id == "3280", |_| false);
        assert_eq!(p.id.as_deref(), Some("3280"));
        assert_eq!(p.name, "Tiam, Hrang & Tial Cin");
        assert_eq!(p.rest, "2025_1040_Tax_Return_TIAM");
        assert_eq!(p.year, Some(2025));
        let p = read_name(name, Some(&re), &off, 2026, |id| id == "TIAM", |_| false);
        assert_eq!(p.id.as_deref(), Some("TIAM"));
        // A two-part ID, as written or run together, before either part alone.
        let both = |id: &str| ["3280_TIAM", "3280", "TIAM"].contains(&id);
        let p = read_name(name, Some(&re), &off, 2026, both, |_| false);
        assert_eq!(p.id.as_deref(), Some("3280_TIAM"));
        assert_eq!(p.rest, "2025_1040_Tax_Return");
        let p = read_name(
            name,
            Some(&re),
            &off,
            2026,
            |id| id == "3280TIAM",
            |_| false,
        );
        assert_eq!(p.id.as_deref(), Some("3280TIAM"));
        // Spaces instead of underscores; the ID at the front.
        let ti = |id: &str| id == "TIAM";
        let p = read_name(
            "Tax Return 2025 1040 TIAM.pdf",
            Some(&re),
            &off,
            2026,
            ti,
            |_| false,
        );
        assert_eq!(p.id.as_deref(), Some("TIAM"));
        let long = "TIAM 2025 1040 Tax Return and a long description.pdf";
        let p = read_name(long, Some(&re), &off, 2026, ti, |_| false);
        assert_eq!(p.id.as_deref(), Some("TIAM"));
        // An ID in the middle of a long name is not looked for.
        let mid = "Some Long Client Name 3280 then a long document name.pdf";
        let p = read_name(mid, Some(&re), &off, 2026, |id| id == "3280", |_| false);
        assert_eq!(p.id, None);
        // No part is a client: as before.
        let p = read_name(name, Some(&re), &off, 2026, |_| false, |_| false);
        assert_eq!(p.id.as_deref(), Some("2025"));
    }

    #[test]
    fn validation() {
        assert!(validate_id_pattern(DEFAULT_ID_PATTERN).is_ok());
        assert!(validate_id_pattern(r"[A-Z]{2}\d{3}").is_ok());
        assert!(validate_id_pattern("").is_err());
        assert!(validate_id_pattern(r"\d*").is_err());
        assert!(validate_id_pattern(r"(\d+").is_err());
        assert!(validate_id_pattern(r"(?P<id>\d+)").is_err());
    }

    #[test]
    fn parses_convention_and_fallback() {
        let re = filename_regex(DEFAULT_ID_PATTERN).unwrap();
        let p = parse_filename("Smith, John_1234_2025_1040 return.PDF", &re, 2026);
        assert_eq!(p.name, "Smith, John");
        assert_eq!(p.id.as_deref(), Some("1234"));
        assert_eq!(p.rest, "2025_1040 return");
        assert_eq!(p.ext, "PDF");
        assert_eq!(p.year, Some(2025));

        let p = parse_filename("scan0042 2024.pdf", &re, 2026);
        assert_eq!(p.id, None);
        assert_eq!(p.name, "scan0042 2024");
        assert_eq!(p.rest, "scan0042 2024");
        assert_eq!(p.ext, "pdf");
        assert_eq!(p.year, Some(2024));

        let p = parse_filename("README", &re, 2026);
        assert_eq!((p.name.as_str(), p.ext.as_str()), ("README", ""));
    }

    #[test]
    fn year_window_and_first_year_wins() {
        assert_eq!(
            detect_year("2025 Organizer for 2024 return", 2026),
            Some(2025)
        );
        assert_eq!(
            detect_year("1040 2021", 2026),
            Some(2021),
            "1040 is outside the window"
        );
        assert_eq!(
            detect_year("2016 notes", 2026),
            Some(2016),
            "10 years back (#41)"
        );
        assert_eq!(detect_year("2015 notes", 2026), None, "11 years back");
        assert_eq!(detect_year("2027 estimate", 2026), Some(2027));
        assert_eq!(detect_year("2028 plan", 2026), None);
        assert_eq!(
            detect_year("ref 202512", 2026),
            None,
            "part of a longer number"
        );
        assert_eq!(detect_year("TY2025_final", 2026), Some(2025));
        assert_eq!(detect_year("no year", 2026), None);
    }
}
