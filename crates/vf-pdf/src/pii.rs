//! Finds SSNs, EINs, account numbers and dates of birth in page text (P04 step 5; Q-27: P04
//! builds this finder, PA-15 reuses it for document flags). Each match maps back to the boxes
//! of its characters, so redaction knows what to cover.

use std::sync::OnceLock;

use regex::Regex;

/// What a match is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Ssn,
    Ein,
    Account,
    Dob,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Ssn, Kind::Ein, Kind::Account, Kind::Dob];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ssn => "SSN",
            Kind::Ein => "EIN",
            Kind::Account => "Account number",
            Kind::Dob => "Date of birth",
        }
    }

    /// Key for audit counts.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Ssn => "ssn",
            Kind::Ein => "ein",
            Kind::Account => "account",
            Kind::Dob => "dob",
        }
    }
}

/// One match on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub page: usize,
    pub kind: Kind,
    /// The matched characters (shown masked in the UI).
    pub value: String,
    /// Character boxes, one per line the match spans (user space).
    pub rects: Vec<[f32; 4]>,
}

impl Found {
    /// `•••-••-4471` / `43-••••912`: never the full number outside the page itself.
    pub fn masked(&self) -> String {
        mask(&self.value)
    }
}

/// Digits but the last 4 (SSN, account) or the first 2 and last 3 (EIN) shown as dots.
pub fn mask(value: &str) -> String {
    let digits = value.chars().filter(char::is_ascii_digit).count();
    let keep_tail = if digits == 9 && value.chars().nth(2) == Some('-') {
        3
    } else {
        4
    };
    let keep_head = if keep_tail == 3 { 2 } else { 0 };
    let mut seen = 0;
    value
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                seen += 1;
                if seen <= keep_head || seen > digits - keep_tail {
                    c
                } else {
                    '•'
                }
            } else {
                c
            }
        })
        .collect()
}

fn patterns() -> &'static [(Kind, Regex, usize)] {
    static P: OnceLock<Vec<(Kind, Regex, usize)>> = OnceLock::new();
    P.get_or_init(|| {
        let r = |s: &str| Regex::new(s).expect("pattern");
        vec![
            // 123-45-6789 / 123 45 6789 (not part of a longer number).
            (Kind::Ssn, r(r"(?:^|[^\d-])(\d{3}[- ]\d{2}[- ]\d{4})(?:$|[^\d-])"), 1),
            (Kind::Ein, r(r"(?:^|[^\d-])(\d{2}-\d{7})(?:$|[^\d-])"), 1),
            // 123456789 right after an SSN label (no separators; QA I11).
            (
                Kind::Ssn,
                r(r"(?i)(?:ssn|social\s+security(?:\s+(?:no\.?|number|#))?|soc\.?\s*sec\.?(?:\s*(?:no\.?|#))?)\W{0,12}(\d{9})(?:$|\D)"),
                1,
            ),
            (
                Kind::Account,
                r(r"(?i)(?:account|acct|a/c)\.?\s*(?:no\.?|number|#)?\s*[:#]?\s*([0-9][0-9 -]{4,}[0-9])"),
                1,
            ),
            (
                Kind::Dob,
                r(r"(?i)(?:dob|d\.o\.b\.|date of birth|birth ?date|born)\s*[:]?\s*(\d{1,2}/\d{1,2}/\d{2,4}|\d{4}-\d{2}-\d{2})"),
                1,
            ),
        ]
    })
}

/// Matches of `kinds` in one page's characters (`chars`: from [`crate::text::chars`]).
pub fn find_on_page(page: usize, chars: &[(char, Option<[f32; 4]>)], kinds: &[Kind]) -> Vec<Found> {
    let text: String = chars.iter().map(|c| c.0).collect();
    // Byte offset → char index.
    let idx: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let char_at = |byte: usize| idx.partition_point(|&b| b < byte);
    let mut out = Vec::new();
    for (kind, re, group) in patterns() {
        if !kinds.contains(kind) {
            continue;
        }
        for caps in re.captures_iter(&text) {
            let Some(m) = caps.get(*group) else { continue };
            let (a, b) = (char_at(m.start()), char_at(m.end()));
            out.push(Found {
                page,
                kind: *kind,
                value: m.as_str().to_string(),
                rects: line_boxes(&chars[a..b]),
            });
        }
    }
    out.sort_by(|x, y| (x.kind, &x.value).cmp(&(y.kind, &y.value)));
    out
}

/// The boxes of a run of characters, one per line (a new line starts when a character is
/// lower than the run so far).
fn line_boxes(run: &[(char, Option<[f32; 4]>)]) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    for b in run.iter().filter_map(|c| c.1) {
        match out.last_mut() {
            Some(cur) if b[3] > cur[1] && b[1] < cur[3] => *cur = crate::text::union(*cur, b),
            _ => out.push(b),
        }
    }
    out
}

/// Whether `text` still holds `value` (digits compared without separators, so a value split
/// by spaces or line breaks is still caught).
pub fn still_contains(text: &str, value: &str) -> bool {
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return text.contains(value);
    }
    let all: String = text.chars().filter(char::is_ascii_digit).collect();
    all.contains(&digits) || text.contains(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QA I11: nine digits after an SSN label, without separators.
    #[test]
    fn ssn_without_separators_after_a_label() {
        let chars = |t: &str| t.chars().map(|c| (c, None)).collect::<Vec<_>>();
        let f = find_on_page(0, &chars("Employee SSN: 987654321"), &[Kind::Ssn]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].value, "987654321");
        assert!(find_on_page(0, &chars("Invoice 987654321"), &[Kind::Ssn]).is_empty());
    }

    fn run(text: &str) -> Vec<(char, Option<[f32; 4]>)> {
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                let x = i as f32 * 6.0;
                (
                    c,
                    (!c.is_whitespace()).then_some([x, 700.0, x + 5.0, 712.0]),
                )
            })
            .collect()
    }

    #[test]
    fn finds_each_kind_with_boxes() {
        let page = run(
            "Recipient SSN: 987-65-4321  Payer EIN: 07-1234567  Acct # 1234 5678 9012  DOB: 04/15/1961",
        );
        let all = find_on_page(2, &page, &Kind::ALL);
        let got: Vec<(Kind, &str)> = all.iter().map(|f| (f.kind, f.value.as_str())).collect();
        assert_eq!(
            got,
            [
                (Kind::Ssn, "987-65-4321"),
                (Kind::Ein, "07-1234567"),
                (Kind::Account, "1234 5678 9012"),
                (Kind::Dob, "04/15/1961"),
            ]
        );
        let ssn = &all[0];
        assert_eq!(ssn.page, 2);
        assert_eq!(ssn.rects.len(), 1);
        assert_eq!(ssn.rects[0][0], 15.0 * 6.0, "starts at the 16th character");
        assert_eq!(ssn.masked(), "•••-••-4321");
        assert_eq!(all[1].masked(), "07-••••567");
        // Only what was asked for.
        assert_eq!(find_on_page(0, &page, &[Kind::Ssn]).len(), 1);
    }

    #[test]
    fn longer_numbers_are_not_ssns() {
        assert!(
            find_on_page(
                0,
                &run("Ref 1987-65-43210 and 12-34567890"),
                &[Kind::Ssn, Kind::Ein]
            )
            .is_empty()
        );
        assert!(still_contains("SSN 987 65\n4321", "987-65-4321"));
        assert!(!still_contains("SSN ███-██-████", "987-65-4321"));
    }
}
