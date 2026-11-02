//! Filename parsing (`ClientName_ID_rest.ext`) and tax-year detection.

use regex::Regex;

/// Default for `Settings::id_pattern`: the ID part of `ClientName_ID_rest.ext`.
pub const DEFAULT_ID_PATTERN: &str = r"\d{4,}";
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
        };
    }
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
