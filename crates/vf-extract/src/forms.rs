//! Form recognizers (PHASES P08 step 1): which information return a page is, its tax year,
//! and its two parties (name and the last 4 digits of the TIN).
//!
//! Input is a page's text in reading order (a PDF's text layer, or Tesseract's output for a
//! scan). The title is found by the form number (tolerating OCR's `O` for `0` and lost
//! hyphens) or the form's printed name; the parties by the IRS box labels, with the value on
//! the label's line or the line after (a label line holding both parties' labels takes its
//! values in the same order).

use std::fmt;

/// The last 4 digits of a TIN. The full number can not be stored in one (Q-08, Q-27).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Last4(String);

impl Last4 {
    /// The last 4 digits of `s`, when it has 4 digits at its end (masked forms keep them).
    pub fn of(s: &str) -> Option<Last4> {
        let digits: Vec<char> = s.chars().filter(|c| c.is_ascii_digit()).collect();
        (digits.len() >= 4).then(|| Last4(digits[digits.len() - 4..].iter().collect()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Last4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "…{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    W2,
    Int,
    Div,
    B,
    R,
    Nec,
    Misc,
    F1098,
    K1Partnership,
    K1SCorp,
    K1Trust,
    Ssa1099,
}

impl Form {
    pub const ALL: [Form; 12] = [
        Form::W2,
        Form::Int,
        Form::Div,
        Form::B,
        Form::R,
        Form::Nec,
        Form::Misc,
        Form::F1098,
        Form::K1Partnership,
        Form::K1SCorp,
        Form::K1Trust,
        Form::Ssa1099,
    ];

    /// The `document_extract.form` value.
    pub fn key(self) -> &'static str {
        match self {
            Form::W2 => "w2",
            Form::Int => "1099-int",
            Form::Div => "1099-div",
            Form::B => "1099-b",
            Form::R => "1099-r",
            Form::Nec => "1099-nec",
            Form::Misc => "1099-misc",
            Form::F1098 => "1098",
            Form::K1Partnership => "k1-1065",
            Form::K1SCorp => "k1-1120s",
            Form::K1Trust => "k1-1041",
            Form::Ssa1099 => "ssa-1099",
        }
    }

    /// The seeded doc type it files as (`vf_fs::doctypes::SEED`).
    pub fn doc_type(self) -> &'static str {
        match self {
            Form::W2 => "W-2",
            Form::Int => "1099-INT",
            Form::Div => "1099-DIV",
            Form::B => "1099-B",
            Form::R => "1099-R",
            Form::Nec => "1099-NEC",
            Form::Misc => "1099-MISC",
            Form::F1098 => "1098",
            Form::K1Partnership => "K-1 (1065)",
            Form::K1SCorp => "K-1 (1120-S)",
            Form::K1Trust => "K-1 (1041)",
            Form::Ssa1099 => "SSA-1099",
        }
    }

    pub fn is_k1(self) -> bool {
        matches!(self, Form::K1Partnership | Form::K1SCorp | Form::K1Trust)
    }

    /// Box labels: (the issuer's TIN, the issuer's name, the client side's TIN, its name).
    /// For a K-1 the issuer is the entity and the client side the partner / shareholder /
    /// beneficiary; for a 1098 the lender issues it and the borrower is the client side.
    fn labels(self) -> Labels {
        match self {
            Form::W2 => Labels {
                payer_tin: &[
                    "EMPLOYER IDENTIFICATION NUMBER",
                    "EMPLOYER'S EIN",
                    "EMPLOYER EIN",
                ],
                payer_name: &["EMPLOYER'S NAME"],
                recip_tin: &["EMPLOYEE'S SOCIAL SECURITY NUMBER", "EMPLOYEE'S SSN"],
                recip_name: &["EMPLOYEE'S FIRST NAME", "EMPLOYEE'S NAME"],
            },
            Form::F1098 => Labels {
                payer_tin: &["RECIPIENT'S/LENDER'S TIN", "LENDER'S TIN"],
                payer_name: &["RECIPIENT'S/LENDER'S NAME", "LENDER'S NAME"],
                recip_tin: &["PAYER'S/BORROWER'S TIN", "BORROWER'S TIN"],
                recip_name: &["PAYER'S/BORROWER'S NAME", "BORROWER'S NAME"],
            },
            Form::K1Partnership => Labels {
                payer_tin: &[
                    "PARTNERSHIP'S EMPLOYER IDENTIFICATION NUMBER",
                    "PARTNERSHIP'S EIN",
                ],
                payer_name: &["PARTNERSHIP'S NAME"],
                recip_tin: &[
                    "PARTNER'S SSN OR TIN",
                    "PARTNER'S IDENTIFYING NUMBER",
                    "PARTNER'S TIN",
                ],
                recip_name: &["PARTNER'S NAME"],
            },
            Form::K1SCorp => Labels {
                payer_tin: &[
                    "CORPORATION'S EMPLOYER IDENTIFICATION NUMBER",
                    "CORPORATION'S EIN",
                ],
                payer_name: &["CORPORATION'S NAME"],
                recip_tin: &["SHAREHOLDER'S IDENTIFYING NUMBER", "SHAREHOLDER'S TIN"],
                recip_name: &["SHAREHOLDER'S NAME"],
            },
            Form::K1Trust => Labels {
                payer_tin: &[
                    "ESTATE'S OR TRUST'S EMPLOYER IDENTIFICATION NUMBER",
                    "TRUST'S EIN",
                ],
                payer_name: &["NAME OF ESTATE OR TRUST", "ESTATE'S OR TRUST'S NAME"],
                recip_tin: &["BENEFICIARY'S IDENTIFYING NUMBER", "BENEFICIARY'S TIN"],
                recip_name: &["BENEFICIARY'S NAME"],
            },
            Form::Ssa1099 => Labels {
                payer_tin: &[],
                payer_name: &[],
                recip_tin: &["BENEFICIARY'S SOCIAL SECURITY NUMBER"],
                recip_name: &["BOX 1. NAME", "BENEFICIARY'S NAME"],
            },
            _ => Labels {
                payer_tin: &["PAYER'S TIN", "PAYER'S FEDERAL IDENTIFICATION NUMBER"],
                payer_name: &["PAYER'S NAME"],
                recip_tin: &["RECIPIENT'S TIN", "RECIPIENT'S IDENTIFICATION NUMBER"],
                recip_name: &["RECIPIENT'S NAME"],
            },
        }
    }
}

struct Labels {
    payer_tin: &'static [&'static str],
    payer_name: &'static [&'static str],
    recip_tin: &'static [&'static str],
    recip_name: &'static [&'static str],
}

/// What a recognizer read from a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Extract {
    pub form: Form,
    pub tax_year: Option<i32>,
    /// The issuer (employer, payer, lender, the K-1's entity).
    pub payer_name: Option<String>,
    pub payer_tin4: Option<Last4>,
    /// The client side (employee, recipient, borrower, partner / shareholder / beneficiary).
    pub recipient_name: Option<String>,
    pub recipient_tin4: Option<Last4>,
    pub corrected: bool,
    /// 0..1: how much of the form was read (title, year, both parties).
    pub confidence: f32,
}

/// Upper case, OCR's common slips in form numbers undone, runs of spaces collapsed.
fn normalize(line: &str) -> String {
    let up = line
        .to_uppercase()
        .replace(['\u{2019}', '`'], "'")
        .replace(['\u{2013}', '\u{2014}'], "-");
    up.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `1O99` → `1099`, `1O65` → `1065` (OCR reads 0 as O inside form numbers).
fn fix_digits(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let near_digit = |j: Option<usize>| {
            j.and_then(|j| chars.get(j))
                .is_some_and(|c| c.is_ascii_digit())
        };
        if c == 'O' && (near_digit(i.checked_sub(1)) && near_digit(Some(i + 1))) {
            out.push('0');
        } else {
            out.push(c);
        }
    }
    out
}

/// Which form the page is, from its first lines.
fn title(head: &str) -> Option<Form> {
    let h = fix_digits(head);
    let compact = h.replace(['-', ' '], "");
    if h.contains("SCHEDULE K-1") || compact.contains("SCHEDULEK1") {
        if compact.contains("1120S") || compact.contains("FORM1120") {
            return Some(Form::K1SCorp);
        }
        if compact.contains("FORM1041") {
            return Some(Form::K1Trust);
        }
        if compact.contains("FORM1065") {
            return Some(Form::K1Partnership);
        }
    }
    if compact.contains("SSA1099") || h.contains("SOCIAL SECURITY BENEFIT STATEMENT") {
        return Some(Form::Ssa1099);
    }
    if h.contains("WAGE AND TAX STATEMENT") || compact.contains("FORMW2") {
        return Some(Form::W2);
    }
    for (code, phrase, form) in [
        ("1099INT", "INTEREST INCOME", Form::Int),
        ("1099DIV", "DIVIDENDS AND DISTRIBUTIONS", Form::Div),
        ("1099B", "PROCEEDS FROM BROKER", Form::B),
        ("1099R", "DISTRIBUTIONS FROM PENSIONS", Form::R),
        ("1099NEC", "NONEMPLOYEE COMPENSATION", Form::Nec),
        ("1099MISC", "MISCELLANEOUS INFORMATION", Form::Misc),
    ] {
        if compact.contains(code) || h.contains(phrase) {
            return Some(form);
        }
    }
    if h.contains("MORTGAGE INTEREST STATEMENT") || compact.contains("FORM1098") {
        return Some(Form::F1098);
    }
    None
}

/// TIN-looking tokens on a line, in order: `123-45-6789`, `12-3456789`, masked `XXX-XX-1234`
/// / `***-**-1234`, or 9 digits run together.
fn tins(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in line.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | ':' | '#' | '=' | '(' | ')' | '/')
    }) {
        let t = tok.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '*'));
        let shape: String = t
            .chars()
            .map(|c| match c {
                '0'..='9' => 'd',
                'X' | 'x' | '*' => 'm',
                '-' => '-',
                _ => '?',
            })
            .collect();
        let masked_or_digit = |s: &str| s.chars().all(|c| c == 'd' || c == 'm');
        let parts: Vec<&str> = shape.split('-').collect();
        let ok = match parts.as_slice() {
            [a, b, c] => {
                a.len() == 3
                    && b.len() == 2
                    && c.len() == 4
                    && [*a, *b].iter().all(|p| masked_or_digit(p))
                    && c.chars().all(|ch| ch == 'd')
            }
            [a, b] => {
                a.len() == 2
                    && b.len() == 7
                    && masked_or_digit(a)
                    && b.ends_with("dddd")
                    && masked_or_digit(b)
            }
            [a] => a.len() == 9 && a.ends_with("dddd") && masked_or_digit(a),
            _ => false,
        };
        if ok {
            out.push(t.to_string());
        }
    }
    out
}

/// Where a label occurs on the normalized lines: (line, column).
fn find_label(lines: &[String], labels: &[&str]) -> Option<(usize, usize)> {
    lines.iter().enumerate().find_map(|(i, l)| {
        labels
            .iter()
            .filter_map(|lab| l.find(lab).map(|c| (c, lab.len())))
            .min()
            .map(|(c, _)| (i, c))
    })
}

/// The TIN for a label: on the label's line after it, or the next line with any; when the
/// label line also holds the other party's TIN label, by order on the line.
fn tin_for(lines: &[String], labels: &[&str], other: &[&str]) -> Option<Last4> {
    let (i, col) = find_label(lines, labels)?;
    let line = &lines[i];
    let label_len = labels
        .iter()
        .filter(|l| line[col..].starts_with(**l))
        .map(|l| l.len())
        .max()
        .unwrap_or(0);
    let after = &line[col + label_len..];
    if let Some(t) = tins(after).first() {
        // Two labels on one line with their values after each: the first after ours.
        return Last4::of(t);
    }
    // Rank among TIN labels on this line (a two-column layout).
    let mut cols: Vec<usize> = vec![col];
    if let Some(o) = other.iter().filter_map(|l| line.find(l)).min() {
        cols.push(o);
    }
    cols.sort_unstable();
    let rank = cols.iter().position(|c| *c == col).unwrap_or(0);
    for l in lines.iter().skip(i + 1).take(3) {
        let found = tins(l);
        if !found.is_empty() {
            return found.get(rank).or(found.first()).and_then(|t| Last4::of(t));
        }
    }
    None
}

/// The name for a label: the rest of its line after a colon, else the next line that is not
/// another label.
fn name_for(lines: &[String], originals: &[String], labels: &[&str]) -> Option<String> {
    let (i, col) = find_label(lines, labels)?;
    // Same characters as the normalized line, original case (ASCII upper-casing keeps
    // positions; anything else falls back to the next line).
    if let Some(rest) = originals[i].get(col..)
        && let Some(p) = rest.find(':')
    {
        let v = clean_name(rest[p + 1..].trim());
        if v.len() > 1 {
            return Some(v);
        }
    }
    for (l, o) in lines.iter().zip(originals).skip(i + 1).take(2) {
        let looks_label = l.contains("'S ") || l.contains("NUMBER") || l.ends_with(':');
        let n = clean_name(o);
        if !l.is_empty() && !looks_label && tins(l).is_empty() && n.len() > 1 {
            return Some(n);
        }
    }
    None
}

/// A name as printed, without any word carrying 5+ digits or a TIN label: a number glued to
/// its label (`SSN:900-12-4321`) can never ride along into a file name or the catalog.
fn clean_name(s: &str) -> String {
    s.split_whitespace()
        .filter(|w| w.chars().filter(|c| c.is_ascii_digit()).count() < 5)
        .filter(|w| {
            let u = w.trim_end_matches([':', '#', '.']).to_ascii_uppercase();
            !matches!(u.as_str(), "SSN" | "EIN" | "TIN" | "FEIN" | "ITIN")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tax year: after "calendar year" / "tax year" / "for", else the most common 20xx in the
/// first lines.
fn year(head: &[String]) -> Option<i32> {
    let mut counts: std::collections::BTreeMap<i32, usize> = Default::default();
    for l in head {
        let l = fix_digits(l);
        for tok in l.split(|c: char| !c.is_ascii_digit()) {
            if tok.len() == 4
                && let Ok(y) = tok.parse::<i32>()
                && (2000..=2100).contains(&y)
            {
                *counts.entry(y).or_default() += 1;
                if l.contains("CALENDAR YEAR") || l.contains("TAX YEAR") {
                    return Some(y);
                }
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(y, n)| (*n, *y))
        .map(|(y, _)| y)
}

/// Reads one page's text. `None` when it is not one of the forms.
pub fn recognize(text: &str) -> Option<Extract> {
    let originals: Vec<String> = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    let lines: Vec<String> = originals.iter().map(|l| normalize(l)).collect();
    let head: Vec<String> = lines.iter().take(12).cloned().collect();
    let form = title(&head.join("\n"))?;
    let lab = form.labels();
    let payer_tin4 = (!lab.payer_tin.is_empty())
        .then(|| tin_for(&lines, lab.payer_tin, lab.recip_tin))
        .flatten();
    let recipient_tin4 = tin_for(&lines, lab.recip_tin, lab.payer_tin);
    let payer_name = (!lab.payer_name.is_empty())
        .then(|| name_for(&lines, &originals, lab.payer_name))
        .flatten();
    let recipient_name = name_for(&lines, &originals, lab.recip_name);
    let tax_year = year(&head);
    let corrected = head
        .iter()
        .any(|l| l.split_whitespace().any(|w| w == "CORRECTED"));
    let parts = [
        tax_year.is_some(),
        recipient_tin4.is_some(),
        recipient_name.is_some(),
        payer_name.is_some() || form == Form::Ssa1099,
    ];
    let confidence = 0.4 + 0.15 * parts.iter().filter(|p| **p).count() as f32;
    Some(Extract {
        form,
        tax_year,
        payer_name,
        payer_tin4,
        recipient_name,
        recipient_tin4,
        corrected,
        confidence: confidence.min(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last4_keeps_four_digits_only() {
        assert_eq!(Last4::of("900-12-3456").unwrap().as_str(), "3456");
        assert_eq!(Last4::of("XXX-XX-1234").unwrap().as_str(), "1234");
        assert!(Last4::of("12").is_none());
        assert_eq!(format!("{:?}", Last4::of("900123456").unwrap()), "…3456");
    }

    #[test]
    fn tin_shapes() {
        assert_eq!(
            tins("a 900-12-3456 b 12-3456789"),
            ["900-12-3456", "12-3456789"]
        );
        assert_eq!(
            tins("XXX-XX-1234 ***-**-5678"),
            ["XXX-XX-1234", "***-**-5678"]
        );
        assert_eq!(tins("900123456"), ["900123456"]);
        assert!(tins("2025 12345 555-1234").is_empty());
    }

    /// QA I2: a TIN glued to its label never reaches a name.
    #[test]
    fn glued_tins_stay_out_of_names() {
        assert_eq!(
            tins("SSN:900-12-4321 EIN#41-1234567"),
            ["900-12-4321", "41-1234567"]
        );
        let t = "Form W-2 Wage and Tax Statement 2025
Employee's name: Dana Reyes SSN:900-12-4321
Employer's name: Harbor Point Farms EIN:41-1234567";
        let e = recognize(t).unwrap();
        for n in [&e.payer_name, &e.recipient_name].into_iter().flatten() {
            assert!(!n.contains("4321") && !n.contains("1234567"), "{n}");
            assert!(!n.contains("SSN") && !n.contains("EIN"), "{n}");
        }
    }

    #[test]
    fn a_plain_w2() {
        let t = "Form W-2 Wage and Tax Statement 2025
a Employee's social security number
XXX-XX-4321
b Employer identification number (EIN)
41-1234567
c Employer's name, address, and ZIP code
Harbor Point Farms LLC
e Employee's first name and initial Last name
Dana Reyes
1 Wages, tips, other compensation 52000.00";
        let e = recognize(t).unwrap();
        assert_eq!(e.form, Form::W2);
        assert_eq!(e.tax_year, Some(2025));
        assert_eq!(e.recipient_tin4.unwrap().as_str(), "4321");
        assert_eq!(e.payer_tin4.unwrap().as_str(), "4567");
        assert_eq!(e.payer_name.as_deref(), Some("Harbor Point Farms LLC"));
        assert_eq!(e.recipient_name.as_deref(), Some("Dana Reyes"));
        assert!(e.confidence > 0.95);
        assert!(recognize("Engagement letter for the 2025 tax year").is_none());
    }
}
