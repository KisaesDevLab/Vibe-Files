//! Synthetic forms with ground truth (PHASES P08 step 5): invented clients and payers, each
//! recognizer's form in several layouts (values on the next line or after a colon, the two
//! parties in either order, both TIN labels on one line, masked TINs, CORRECTED, OCR slips
//! such as `1O99` and lost hyphens), and pages that are not forms. Every name and number is
//! made up; the full TINs exist only here, so tests can check none of them is ever stored.

use crate::forms::Form;
use crate::matching::{Alias, ClientRef};

/// An invented client: (CSV ID, name, full TIN).
pub const CLIENTS: [(&str, &str, &str); 9] = [
    ("ANDE1042", "Anderson Farms LLC", "41-1234567"),
    ("REYE5521", "Reyes, Dana & Lee", "900-11-4321"),
    ("CEDA7788", "Cedar Ridge Partners", "52-7654321"),
    ("ORTI3300", "Ortiz, Sam", "900-22-8765"),
    ("BELL2002", "Bell Ranch Inc", "63-1112222"),
    ("WAGN1500", "Wagner, Pat", "900-33-1357"),
    ("KOHL4400", "Kohler Family Trust", "74-3334444"),
    ("PARK6100", "Park, Jordan", "900-44-2468"),
    // Same last 4 as Park: only the name tells them apart.
    ("LEEJ2200", "Lee, Jamie", "900-55-2468"),
];

/// Invented payers that are not clients: (name, EIN).
pub const PAYERS: [(&str, &str); 4] = [
    ("Northwind Bank NA", "81-0001111"),
    ("Summit Brokerage LLC", "82-0002222"),
    ("Lakeside Mortgage Co", "83-0003333"),
    ("Riverbend Holdings LP", "84-0004444"),
];

/// Every full TIN the fixtures use (for leak checks), with and without hyphens.
pub fn full_tins() -> Vec<String> {
    CLIENTS
        .iter()
        .map(|c| c.2)
        .chain(PAYERS.iter().map(|p| p.1))
        .flat_map(|t| [t.to_string(), t.replace('-', "")])
        .collect()
}

/// The client list as the matcher sees it (uuid = `id-<CSV ID>`; last 4 only).
pub fn client_refs() -> Vec<ClientRef> {
    CLIENTS
        .iter()
        .map(|(ext, name, tin)| ClientRef {
            id: format!("id-{ext}"),
            ext_id: ext.to_string(),
            name: name.to_string(),
            tin4: crate::forms::Last4::of(tin).map(|l| l.as_str().to_string()),
        })
        .collect()
}

pub fn aliases() -> Vec<Alias> {
    vec![]
}

/// What a page should be read as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truth {
    pub form: Form,
    pub year: i32,
    /// CSV ID of the primary destination.
    pub primary: String,
    /// CSV IDs of copies (a K-1's partner when the entity is a client).
    pub copies: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub text: String,
    /// `None`: not a form.
    pub truth: Option<Truth>,
}

/// How a form is laid out.
#[derive(Debug, Clone, Copy)]
struct Style {
    /// Values after a colon on the label's line (else on the next line).
    inline: bool,
    /// The client side's block first.
    swapped: bool,
    /// Both TIN labels on one line, values on the next.
    two_column: bool,
    /// The client side's TIN masked (`XXX-XX-1234`).
    masked: bool,
    corrected: bool,
    /// OCR slips: `1O99`, TINs without hyphens.
    noisy: bool,
}

impl Style {
    fn nth(i: usize) -> Style {
        Style {
            inline: i % 2 == 1,
            swapped: i % 3 == 2,
            two_column: i % 4 == 3,
            masked: i % 2 == 0,
            corrected: i % 7 == 5,
            noisy: i % 5 == 4,
        }
    }
}

fn tin(t: &str, masked: bool, noisy: bool) -> String {
    let t = if masked {
        // Keep the shape, hide all but the last 4.
        let n = t.len();
        t.chars()
            .enumerate()
            .map(|(i, c)| {
                if i < n - 4 && c.is_ascii_digit() {
                    'X'
                } else {
                    c
                }
            })
            .collect()
    } else {
        t.to_string()
    };
    if noisy { t.replace('-', "") } else { t }
}

/// One party's lines: TIN label and value, name label and value.
fn party(
    lines: &mut Vec<String>,
    tin_label: &str,
    tin_value: &str,
    name_label: &str,
    name: &str,
    s: Style,
) {
    if s.inline {
        lines.push(format!("{tin_label}: {tin_value}"));
        lines.push(format!("{name_label}: {name}"));
    } else {
        lines.push(tin_label.to_string());
        lines.push(tin_value.to_string());
        lines.push(format!("{name_label}, street address, city or town"));
        lines.push(name.to_string());
    }
}

struct Parties<'a> {
    payer_tin_label: &'a str,
    payer_name_label: &'a str,
    recip_tin_label: &'a str,
    recip_name_label: &'a str,
}

fn parties(form: Form) -> Parties<'static> {
    match form {
        Form::W2 => Parties {
            payer_tin_label: "b Employer identification number (EIN)",
            payer_name_label: "c Employer's name",
            recip_tin_label: "a Employee's social security number",
            recip_name_label: "e Employee's first name and initial Last name",
        },
        Form::F1098 => Parties {
            payer_tin_label: "RECIPIENT'S/LENDER'S TIN",
            payer_name_label: "RECIPIENT'S/LENDER'S name",
            recip_tin_label: "PAYER'S/BORROWER'S TIN",
            recip_name_label: "PAYER'S/BORROWER'S name",
        },
        Form::K1Partnership => Parties {
            payer_tin_label: "A Partnership's employer identification number",
            payer_name_label: "B Partnership's name",
            recip_tin_label: "E Partner's SSN or TIN",
            recip_name_label: "F Partner's name",
        },
        Form::K1SCorp => Parties {
            payer_tin_label: "A Corporation's employer identification number",
            payer_name_label: "B Corporation's name",
            recip_tin_label: "E Shareholder's identifying number",
            recip_name_label: "F Shareholder's name",
        },
        Form::K1Trust => Parties {
            payer_tin_label: "A Estate's or trust's employer identification number",
            payer_name_label: "B Estate's or trust's name",
            recip_tin_label: "C Beneficiary's identifying number",
            recip_name_label: "D Beneficiary's name",
        },
        Form::Ssa1099 => Parties {
            payer_tin_label: "",
            payer_name_label: "",
            recip_tin_label: "Box 2. Beneficiary's Social Security Number",
            recip_name_label: "Box 1. Name",
        },
        _ => Parties {
            payer_tin_label: "PAYER'S TIN",
            payer_name_label: "PAYER'S name",
            recip_tin_label: "RECIPIENT'S TIN",
            recip_name_label: "RECIPIENT'S name",
        },
    }
}

fn title(form: Form, year: i32, s: Style) -> Vec<String> {
    let year_line = if s.inline {
        format!("For calendar year {year}")
    } else {
        year.to_string()
    };
    let t = match form {
        Form::W2 => "Form W-2 Wage and Tax Statement".to_string(),
        Form::Int => "Form 1099-INT Interest Income".into(),
        Form::Div => "Form 1099-DIV Dividends and Distributions".into(),
        Form::B => "Form 1099-B Proceeds From Broker and Barter Exchange Transactions".into(),
        Form::R => "Form 1099-R Distributions From Pensions, Annuities, Retirement".into(),
        Form::Nec => "Form 1099-NEC Nonemployee Compensation".into(),
        Form::Misc => "Form 1099-MISC Miscellaneous Information".into(),
        Form::F1098 => "Form 1098 Mortgage Interest Statement".into(),
        Form::K1Partnership => {
            "Schedule K-1 (Form 1065) Partner's Share of Income, Deductions, Credits".into()
        }
        Form::K1SCorp => "Schedule K-1 (Form 1120-S) Shareholder's Share of Income".into(),
        Form::K1Trust => "Schedule K-1 (Form 1041) Beneficiary's Share of Income".into(),
        Form::Ssa1099 => "FORM SSA-1099 - SOCIAL SECURITY BENEFIT STATEMENT".into(),
    };
    let t = if s.noisy {
        t.replace("1099", "1O99").replace("1065", "1O65")
    } else {
        t
    };
    let mut v = vec![];
    if s.corrected {
        v.push("CORRECTED (if checked)".into());
    }
    v.push(t);
    v.push(year_line);
    v.push("Department of the Treasury - Internal Revenue Service".into());
    v
}

/// One form page's text.
fn render(form: Form, year: i32, payer: (&str, &str), recip: (&str, &str), s: Style) -> String {
    let p = parties(form);
    let mut lines = title(form, year, s);
    let ptin = tin(payer.1, false, s.noisy);
    let rtin = tin(recip.1, s.masked, s.noisy);
    let has_payer = !p.payer_tin_label.is_empty();
    if s.two_column && has_payer && !s.inline {
        lines.push(format!(
            "{}        {}",
            p.payer_tin_label, p.recip_tin_label
        ));
        lines.push(format!("{ptin}        {rtin}"));
        lines.push(format!("{}, street address", p.payer_name_label));
        lines.push(payer.0.to_string());
        lines.push(format!("{}, street address", p.recip_name_label));
        lines.push(recip.0.to_string());
    } else {
        let mut blocks: Vec<Box<dyn Fn(&mut Vec<String>)>> = vec![];
        if has_payer {
            let (l1, l2, n) = (p.payer_tin_label, p.payer_name_label, payer.0.to_string());
            let t = ptin.clone();
            blocks.push(Box::new(move |v: &mut Vec<String>| {
                party(v, l1, &t, l2, &n, s)
            }));
        }
        let (l1, l2, n) = (p.recip_tin_label, p.recip_name_label, recip.0.to_string());
        let t = rtin.clone();
        blocks.push(Box::new(move |v: &mut Vec<String>| {
            party(v, l1, &t, l2, &n, s)
        }));
        if s.swapped {
            blocks.reverse();
        }
        for b in blocks {
            b(&mut lines);
        }
    }
    lines.push("1 Amount 1,234.56".into());
    lines.push("Keep for your records".into());
    lines.join("\n")
}

fn client(ext: &str) -> (&'static str, &'static str) {
    let c = CLIENTS.iter().find(|c| c.0 == ext).expect("fixture client");
    (c.1, c.2)
}

/// The whole set: about 120 forms and some pages that are not forms.
pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut add = |form: Form,
                   year: i32,
                   payer: (&str, &str),
                   recip_ext: &str,
                   primary: &str,
                   copies: Vec<String>| {
        let s = Style::nth(i);
        i += 1;
        out.push(Case {
            name: format!("{} {year} {recip_ext} #{i}", form.key()),
            text: render(form, year, payer, client(recip_ext), s),
            truth: Some(Truth {
                form,
                year,
                primary: primary.to_string(),
                copies,
            }),
        });
    };
    let individuals = ["REYE5521", "ORTI3300", "WAGN1500", "PARK6100", "LEEJ2200"];
    let entities = ["ANDE1042", "CEDA7788", "BELL2002"];
    let anderson = ("Anderson Farms LLC", "41-1234567");
    for (k, ext) in individuals.iter().enumerate() {
        for year in [2024, 2025] {
            add(Form::W2, year, anderson, ext, ext, vec![]);
            add(Form::Int, year, PAYERS[0], ext, ext, vec![]);
            add(Form::Div, year, PAYERS[1], ext, ext, vec![]);
            add(Form::B, year, PAYERS[1], ext, ext, vec![]);
            add(Form::R, year, PAYERS[1], ext, ext, vec![]);
            add(Form::F1098, year, PAYERS[2], ext, ext, vec![]);
            add(Form::Ssa1099, year, ("", ""), ext, ext, vec![]);
            if k % 2 == 0 {
                add(Form::Nec, year, anderson, ext, ext, vec![]);
                add(Form::Misc, year, anderson, ext, ext, vec![]);
            }
        }
    }
    for ext in entities {
        for year in [2024, 2025] {
            add(Form::Int, year, PAYERS[0], ext, ext, vec![]);
            add(Form::Nec, year, PAYERS[1], ext, ext, vec![]);
        }
    }
    // K-1s: both parties clients (entity primary, partner copy), and from a non-client.
    for year in [2024, 2025] {
        add(
            Form::K1Partnership,
            year,
            ("Cedar Ridge Partners", "52-7654321"),
            "ORTI3300",
            "CEDA7788",
            vec!["ORTI3300".into()],
        );
        add(
            Form::K1SCorp,
            year,
            ("Bell Ranch Inc", "63-1112222"),
            "WAGN1500",
            "BELL2002",
            vec!["WAGN1500".into()],
        );
        add(
            Form::K1Trust,
            year,
            ("Kohler Family Trust", "74-3334444"),
            "PARK6100",
            "KOHL4400",
            vec!["PARK6100".into()],
        );
        add(
            Form::K1Partnership,
            year,
            PAYERS[3],
            "REYE5521",
            "REYE5521",
            vec![],
        );
    }
    for (n, text) in [
        (
            "engagement letter",
            "Engagement Letter\nDear Dana Reyes,\nThis letter confirms our engagement for the 2025 tax year.",
        ),
        ("blank scan", ""),
        (
            "invoice",
            "INVOICE 2025-118\nBill to: Bell Ranch Inc\nTax preparation 1,200.00",
        ),
        (
            "organizer",
            "2025 Tax Organizer\nTaxpayer: Pat Wagner\nPlease bring your W-2 and 1099 forms.",
        ),
    ] {
        out.push(Case {
            name: n.into(),
            text: text.into(),
            truth: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::propose;

    /// PHASES P08 done-when 1: at least 95% correct client + year + doc type on the synthetic
    /// set (computed here, not claimed); pages that are not forms are never read as one.
    #[test]
    fn accuracy_on_the_synthetic_set() {
        let clients = client_refs();
        let (mut right, mut total) = (0usize, 0usize);
        let mut misses = Vec::new();
        for c in cases() {
            let got = crate::forms::recognize(&c.text);
            let Some(t) = c.truth else {
                assert!(got.is_none(), "{} read as a form: {got:?}", c.name);
                continue;
            };
            total += 1;
            let ok = got.as_ref().is_some_and(|e| {
                let p = propose(e, &clients, &aliases());
                let copies: Vec<String> = p.copies.iter().map(|m| m.ext_id.clone()).collect();
                e.form == t.form
                    && e.tax_year == Some(t.year)
                    && p.confident()
                    && p.primary.as_ref().map(|m| m.ext_id.as_str()) == Some(t.primary.as_str())
                    && copies == t.copies
            });
            if ok {
                right += 1;
            } else {
                misses.push(format!(
                    "{}: {:?} {:?}",
                    c.name,
                    got,
                    got.as_ref().map(|e| propose(e, &clients, &aliases()))
                ));
            }
        }
        let rate = right as f32 / total as f32;
        eprintln!("{right}/{total} = {:.1}%", rate * 100.0);
        assert!(total >= 100, "{total}");
        assert!(rate >= 0.95, "{rate}\n{}", misses.join("\n"));
    }

    /// PHASES P08 done-when 2: a K-1 with no ID in its file name proposes both the entity and
    /// the shareholder.
    #[test]
    fn k1_proposes_entity_and_shareholder() {
        let k1 = cases()
            .into_iter()
            .find(|c| c.name.starts_with("k1-1120s"))
            .unwrap();
        let e = crate::forms::recognize(&k1.text).unwrap();
        let p = propose(&e, &client_refs(), &[]);
        assert_eq!(p.doc_type, "K-1 (1120-S)");
        assert_eq!(p.primary.unwrap().ext_id, "BELL2002");
        assert_eq!(p.copies.len(), 1);
        assert_eq!(p.copies[0].ext_id, "WAGN1500");
    }

    /// Two clients with the same last 4: the name decides; with a name that fits neither, no
    /// confident proposal.
    #[test]
    fn same_last4_needs_the_name() {
        let clients = client_refs();
        let text = "Form 1099-INT Interest Income\n2025\nRECIPIENT'S TIN\nXXX-XX-2468\nRECIPIENT'S name\nJamie Lee";
        let e = crate::forms::recognize(text).unwrap();
        let p = propose(&e, &clients, &[]);
        assert_eq!(p.primary.as_ref().unwrap().ext_id, "LEEJ2200");
        assert!(p.confident());
        let e = crate::forms::recognize(&text.replace("Jamie Lee", "Chris Doe")).unwrap();
        assert!(!propose(&e, &clients, &[]).confident());
    }

    /// A learned alias (a correction at filing) makes a payer's forms go to its client.
    #[test]
    fn an_alias_from_a_correction_is_used() {
        let clients = client_refs();
        let text = "Form 1099-NEC Nonemployee Compensation\n2025\nPAYER'S name\nSummit Brokerage LLC\nRECIPIENT'S name\nBell Ranch";
        let e = crate::forms::recognize(text).unwrap();
        // No TIN, a partial name: not sure.
        let p = propose(&e, &clients, &[]);
        assert!(!p.confident(), "{p:?}");
        let learned = [Alias {
            payer: "SUMMIT BROKERAGE".into(),
            client_id: "id-BELL2002".into(),
        }];
        let p = propose(&e, &clients, &learned);
        assert!(p.confident(), "{p:?}");
        assert_eq!(p.primary.unwrap().ext_id, "BELL2002");
    }

    /// Nothing the recognizers return can hold a full TIN.
    #[test]
    fn extracts_never_hold_a_full_tin() {
        for c in cases() {
            if let Some(e) = crate::forms::recognize(&c.text) {
                let dump = format!("{e:?}");
                for full in full_tins() {
                    assert!(!dump.contains(&full), "{}: {dump}", c.name);
                }
            }
        }
    }
}
