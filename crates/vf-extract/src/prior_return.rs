//! A to-do list from last year's return (PA-104, owner 2026-11-23): the documents and answers to
//! ask a client for, read from the text of the UltraTax CS return PDF the firm filed for them.
//! No AI: UltraTax prints each source document as one row of a named supporting report or
//! statement (owner: "for some items it might be better to read off the supporting schedules
//! than a tax form"), and those rows are read by fixed rules:
//!
//! - **Salaries & Wages Report**: `T EMPLOYER 21,500 …` → a W-2 from each employer.
//! - **IRA Distribution Report**, **Pension/Annuity Report**: `S PAYER 7 2,500 …` → a 1099-R.
//! - **Broker Capital Transactions Detail Summary Report**: `BROKER 4400-1234 J` → a 1099-B
//!   (a consolidated 1099 when the same broker paid dividends).
//! - **Federal Statements**: "Form 1040, Dividend Income" (1099-DIV payers), "… Interest Income"
//!   (1099-INT), "Schedule A, Line 8a … Form 1098" (lenders), "Statement n - Schedule E, Page 2,
//!   Line 28" (K-1 entities, P or S), "Form 1040, Line 26 - Estimated Tax Payments",
//!   "Form 6252 - Installment Sale History" (the property is the line above), "Personal Property
//!   Tax" (vehicles).
//! - Forms present: the Social Security Worksheet (SSA-1099), Schedule C, Schedule E Part I,
//!   Schedule F, Form 5695, Form 8863 (1098-T), Form 8962 (1095-A), Form 2441.
//!
//! Pure: page text in (as pdfium gives it, one line per printed row), proposed items out. Names
//! are shown as the return prints them, in title case; amounts are never carried into an item,
//! and an account number only by its last 4 digits.

use std::collections::BTreeSet;

/// What an item asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The client sends files.
    Document,
    /// The client answers in words.
    Answer,
}

/// A heading the items are shown under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Wages,
    Retirement,
    Investments,
    PassThrough,
    Home,
    Business,
    Other,
    Questions,
}

impl Group {
    pub fn words(self) -> &'static str {
        match self {
            Group::Wages => "Wages",
            Group::Retirement => "Retirement and Social Security",
            Group::Investments => "Interest, dividends and brokers",
            Group::PassThrough => "K-1s",
            Group::Home => "Home",
            Group::Business => "Business and rentals",
            Group::Other => "Other",
            Group::Questions => "Questions",
        }
    }
}

/// One proposed to-do item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposed {
    pub group: Group,
    pub kind: Kind,
    pub prompt: String,
    /// A doc type name as Vibe Files seeds them ("W-2", "1099-R", …); empty for answers.
    pub doc_type: String,
    /// Where in the return it was found (for the person reviewing).
    pub source: String,
}

/// What was read from a return.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proposal {
    /// The return's tax year, when found; the list asks for the year after.
    pub year: Option<i32>,
    pub taxpayer: Option<String>,
    pub spouse: Option<String>,
    pub items: Vec<Proposed>,
}

impl Proposal {
    /// The year the list asks about (the year after the return's).
    pub fn ask_year(&self) -> Option<i32> {
        self.year.map(|y| y + 1)
    }

    /// Nothing an UltraTax return would have was found (not a return, or a scan).
    pub fn looks_empty(&self) -> bool {
        self.items.iter().all(|i| i.group == Group::Questions)
    }
}

// ---- Text helpers -----------------------------------------------------------------------------

fn lines(page: &str) -> Vec<&str> {
    page.lines().collect()
}

/// A number or money token: `21,500`, `$`, `9.770`, `(1,200)`, `-15`.
fn is_amount(t: &str) -> bool {
    // "(4321)": an account's last digits, not a negative amount (that has a comma or is short).
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')'))
        && inner.len() >= 4
        && inner.chars().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    !t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_digit() || ",.$()-%".contains(c))
        && (t.chars().any(|c| c.is_ascii_digit()) || t == "$")
}

/// `name` without the amounts printed after it on the same row.
fn strip_amounts(name: &str) -> String {
    let mut toks: Vec<&str> = name.split_whitespace().collect();
    while toks.last().is_some_and(|t| is_amount(t)) {
        toks.pop();
    }
    toks.join(" ")
}

const UPPER_KEEP: &[&str] = &[
    "LLC", "LLP", "LP", "PC", "P.C.", "PA", "P.A.", "NA", "N.A.", "USA", "US", "IRA", "FSB",
    "PLLC", "II", "III", "IV", "CPA", "HSA", "UBS", "TD", "SSA", "UMB", "BMO", "PNC",
];

/// "EXAMPLE SECURITIES & CO INC" → "Example Securities & Co Inc"; a mixed-case name stays as printed.
pub fn title_case(s: &str) -> String {
    if s.chars().any(|c| c.is_ascii_lowercase()) {
        return s.to_string();
    }
    s.split(' ')
        .enumerate()
        .map(|(i, w)| {
            if i > 0 && matches!(w, "OF" | "AND" | "THE" | "FOR" | "IN" | "AT" | "ON") {
                return w.to_lowercase();
            }
            if UPPER_KEEP.contains(&w.trim_end_matches(',')) {
                return w.to_string();
            }
            // Short words with no vowel are initials (GM, BMW, VW).
            if w.len() <= 3
                && w.chars().all(|c| c.is_ascii_uppercase())
                && !w.chars().any(|c| "AEIOUY".contains(c))
            {
                return w.to_string();
            }
            // Each part of R-I, O'NEIL, SMITH-JONES.
            let mut out = String::new();
            let mut start = true;
            for c in w.chars() {
                if c.is_alphabetic() {
                    if start {
                        out.extend(c.to_uppercase());
                    } else {
                        out.extend(c.to_lowercase());
                    }
                    start = false;
                } else {
                    out.push(c);
                    start = c == '-' || c == '\'' || c == '(' || c == '/';
                }
            }
            // Single letters (R-I) keep their capitals.
            if w.split('-')
                .all(|p| p.chars().filter(|c| c.is_alphabetic()).count() <= 1)
            {
                return w.to_string();
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A payer name for matching one report against another ("EXAMPLE SECURITIES & CO INC (4321)" and
/// "EXAMPLE SECURITIES & CO INC" are the same).
fn key(name: &str) -> String {
    let n: String = name
        .split('(')
        .next()
        .unwrap_or(name)
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_uppercase();
    n.split_whitespace()
        .filter(|w| !matches!(*w, "INC" | "CO" | "LLC" | "NA" | "CORP" | "COMPANY" | "THE"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// "(4321)" at the end of a name → ("NAME", Some("4321")).
fn account_suffix(name: &str) -> (String, Option<String>) {
    let n = name.trim();
    if let Some(open) = n.rfind('(')
        && n.ends_with(')')
    {
        let inner = &n[open + 1..n.len() - 1];
        let digits: String = inner.chars().filter(char::is_ascii_digit).collect();
        if digits.len() >= 3
            && digits.len() == inner.chars().filter(|c| c.is_alphanumeric()).count()
        {
            let last: String = digits
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return (n[..open].trim().to_string(), Some(last));
        }
    }
    (n.to_string(), None)
}

/// "Bob & Jane Jones" → (Bob, Jane): the line above "Name" / "Name(s) shown on return".
fn names(pages: &[String]) -> (Option<String>, Option<String>) {
    for p in pages {
        let ls = lines(p);
        for (i, l) in ls.iter().enumerate() {
            let t = l.trim();
            if (t == "Name" || t == "Name(s) shown on return") && i > 0 {
                let above = ls[i - 1].trim();
                if above.is_empty() || above.starts_with("Pg ") {
                    continue;
                }
                let mut parts = above.splitn(2, " & ");
                let first = parts
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .map(str::to_string);
                let second = parts
                    .next()
                    .and_then(|s| s.split_whitespace().next())
                    .map(str::to_string);
                return (first.filter(|s| !s.is_empty()), second);
            }
        }
    }
    (None, None)
}

/// The return's tax year: "Taxable Year Ended December 31, 2025", or a report heading
/// "1040 … Report 2025" / "Federal Return Summary 2025".
fn year(pages: &[String]) -> Option<i32> {
    let num = |s: &str| {
        s.trim()
            .trim_end_matches(['.', ','])
            .parse::<i32>()
            .ok()
            .filter(|y| (2000..2100).contains(y))
    };
    for p in pages {
        for l in p.lines() {
            if let Some(rest) = l.split("Taxable Year Ended December 31,").nth(1)
                && let Some(y) = rest.split_whitespace().next().and_then(num)
            {
                return Some(y);
            }
        }
    }
    for p in pages {
        for l in p.lines() {
            let t = l.trim();
            if (t.contains("Federal Return Summary")
                || t.contains("Salaries & Wages Report")
                || t.contains("Report"))
                && t.starts_with("1040")
                && let Some(y) = t.split_whitespace().last().and_then(num)
            {
                return Some(y);
            }
        }
    }
    None
}

fn page_has(p: &str, heading: &str) -> bool {
    p.lines().any(|l| l.contains(heading))
}

/// Rows `T NAME …` / `S NAME …` of a report page: (who, name), the amounts and a trailing box
/// code ("7", "G", "4D") left off.
fn ts_rows(p: &str, code_column: bool) -> Vec<(char, String)> {
    let mut out = Vec::new();
    for l in p.lines() {
        let t = l.trim();
        let mut cs = t.chars();
        let (Some(who), Some(' ')) = (cs.next(), cs.next()) else {
            continue;
        };
        if who != 'T' && who != 'S' {
            continue;
        }
        let mut name = strip_amounts(&t[2..]);
        if code_column {
            let mut toks: Vec<&str> = name.split_whitespace().collect();
            if toks.len() > 1
                && let Some(last) = toks.last()
                && last.len() <= 2
                && (last.chars().any(|c| c.is_ascii_digit()) || last.len() == 1)
            {
                toks.pop();
            }
            name = toks.join(" ");
        }
        // A real name: letters, and not a column heading.
        if name.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !name.starts_with("/S") {
            out.push((who, name));
        }
    }
    out
}

/// The entries of a Federal Statements section: the lines after `heading` up to its TOTAL.
fn section<'a>(p: &'a str, heading: &str) -> Option<Vec<&'a str>> {
    let ls = lines(p);
    let start = ls.iter().position(|l| l.trim().starts_with(heading))?;
    let mut out = Vec::new();
    for l in &ls[start + 1..] {
        if l.trim().starts_with("TOTAL") {
            break;
        }
        out.push(*l);
    }
    Some(out)
}

/// The payer lines of a statement whose rows are a name then its amounts, on the same line
/// ("SAMPLE MORTGAGE CO $ 8,100") or the next ("SAMPLE FUND (4321)" / "$ 120 $ 80").
fn statement_names(entries: &[&str]) -> Vec<String> {
    entries
        .iter()
        .map(|l| strip_amounts(l.trim()))
        .filter(|n| {
            let letters = n.chars().filter(|c| c.is_alphabetic()).count();
            letters >= 2
                && !matches!(
                    n.as_str(),
                    "Payer" | "Description Amount" | "Description" | "Amount" | "Name"
                )
                && !n.starts_with("Ordinary")
                && !n.starts_with("Dividends")
                && !n.starts_with("Interest")
                && !n.starts_with("Tax-Exempt")
        })
        .collect()
}

// ---- The rules ------------------------------------------------------------------------------

struct Out {
    items: Vec<Proposed>,
    seen: BTreeSet<String>,
}

impl Out {
    fn push(&mut self, group: Group, kind: Kind, prompt: String, doc_type: &str, source: &str) {
        if self.seen.insert(prompt.to_lowercase()) {
            self.items.push(Proposed {
                group,
                kind,
                prompt,
                doc_type: doc_type.into(),
                source: source.into(),
            });
        }
    }
}

/// Reads the pages of an UltraTax CS individual return and proposes a to-do list.
pub fn propose(pages: &[String]) -> Proposal {
    let year = year(pages);
    let (taxpayer, spouse) = names(pages);
    let next = year.map_or_else(|| "this year".to_string(), |y| (y + 1).to_string());
    let whose = |who: char| -> String {
        let n = if who == 'S' {
            spouse.as_deref()
        } else {
            taxpayer.as_deref()
        };
        n.map(|n| format!(" ({n})")).unwrap_or_default()
    };
    let mut out = Out {
        items: Vec::new(),
        seen: BTreeSet::new(),
    };

    // Brokers first: a broker that also paid dividends or interest is one consolidated 1099.
    let mut brokers: Vec<(String, Option<String>)> = Vec::new();
    for p in pages
        .iter()
        .filter(|p| page_has(p, "Broker Capital Transactions Detail Summary Report"))
    {
        for l in p.lines() {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let n = toks.len();
            if n >= 3
                && matches!(toks[n - 1], "T" | "S" | "J")
                && toks[n - 2].chars().filter(char::is_ascii_digit).count() >= 4
                && toks[n - 2]
                    .chars()
                    .all(|c| c.is_ascii_digit() || "-*X".contains(c))
            {
                let acct: String = toks[n - 2].chars().filter(char::is_ascii_digit).collect();
                let last4 = acct[acct.len().saturating_sub(4)..].to_string();
                let name = toks[..n - 2].join(" ");
                if name.chars().filter(|c| c.is_alphabetic()).count() >= 2
                    && !brokers
                        .iter()
                        .any(|(b, a)| key(b) == key(&name) && a.as_deref() == Some(&last4))
                {
                    brokers.push((name, Some(last4)));
                }
            }
        }
    }
    let broker_keys: BTreeSet<String> = brokers.iter().map(|(b, _)| key(b)).collect();
    let acct = |a: &Option<String>| {
        a.as_ref()
            .map(|a| format!(", account ending {a}"))
            .unwrap_or_default()
    };

    for p in pages {
        // Wages.
        if page_has(p, "Salaries & Wages Report") {
            for (who, name) in ts_rows(p, false) {
                out.push(
                    Group::Wages,
                    Kind::Document,
                    format!("W-2 from {}{}", title_case(&name), whose(who)),
                    "W-2",
                    "Salaries & Wages Report",
                );
            }
        }
        // Retirement.
        for (report, what) in [
            ("IRA Distribution Report", "IRA"),
            ("Pension/Annuity Report", "pension or annuity"),
        ] {
            if page_has(p, report) {
                for (who, name) in ts_rows(p, true) {
                    out.push(
                        Group::Retirement,
                        Kind::Document,
                        format!("1099-R ({what}) from {}{}", title_case(&name), whose(who)),
                        "1099-R",
                        report,
                    );
                }
            }
        }
        if !page_has(p, "Federal Statements") {
            continue;
        }
        // Dividends and interest (a broker's goes with its consolidated 1099 below).
        for (heading, form) in [
            ("Form 1040, Dividend Income", "1099-DIV"),
            ("Form 1040, Interest Income", "1099-INT"),
            ("Schedule B, Line 1", "1099-INT"),
            ("Schedule B, Line 5", "1099-DIV"),
        ] {
            if let Some(entries) = section(p, heading) {
                for raw in statement_names(&entries) {
                    let (name, last4) = account_suffix(&raw);
                    if broker_keys.contains(&key(&name)) {
                        continue;
                    }
                    out.push(
                        Group::Investments,
                        Kind::Document,
                        format!("{form} from {}{}", title_case(&name), acct(&last4)),
                        form,
                        heading,
                    );
                }
            }
        }
        // Mortgage lenders.
        if let Some(entries) = section(p, "Schedule A, Line 8a") {
            for name in statement_names(&entries) {
                out.push(
                    Group::Home,
                    Kind::Document,
                    format!("Form 1098 (mortgage interest) from {}", title_case(&name)),
                    "1098",
                    "Schedule A, Line 8a",
                );
            }
        }
        // K-1s: a name line, then its P/S and EIN line; indented activity lines are not entities.
        let ls = lines(p);
        if ls.iter().any(|l| l.contains("Schedule E, Page 2, Line 28")) {
            for w in ls.windows(2) {
                let (name, next_line) = (w[0], w[1].trim());
                let ein = next_line.starts_with("P **-")
                    || next_line.starts_with("S **-")
                    || next_line.starts_with("P XX-")
                    || next_line.starts_with("S XX-");
                if ein && !name.starts_with(' ') && !name.trim().is_empty() {
                    let (entity, doc) = if next_line.starts_with('S') {
                        ("S corporation", "K-1 (1120-S)")
                    } else {
                        ("partnership", "K-1 (1065)")
                    };
                    out.push(
                        Group::PassThrough,
                        Kind::Document,
                        format!("K-1 from {} ({entity})", title_case(name.trim())),
                        doc,
                        "Schedule E, Page 2, Line 28",
                    );
                }
            }
        }
        // Estimated tax payments.
        if ls.iter().any(|l| {
            l.trim()
                .starts_with("Form 1040, Line 26 - Estimated Tax Payments")
        }) {
            out.push(
                Group::Questions,
                Kind::Answer,
                format!(
                    "The dates and amounts of your {next} federal and state estimated tax payments"
                ),
                "",
                "Form 1040, Line 26",
            );
        }
        // Installment sales: the property is printed above the heading.
        for (i, l) in ls.iter().enumerate() {
            if l.trim().starts_with("Form 6252 - Installment Sale History") && i > 0 {
                let property = ls[i - 1].trim();
                if !property.is_empty() && !property.contains("Federal Statements") {
                    out.push(
                        Group::Other,
                        Kind::Answer,
                        format!("Payments you received in {next} on the installment sale of {property} (principal and interest)"),
                        "",
                        "Form 6252",
                    );
                }
            }
        }
        // Personal property tax (the vehicles listed).
        if let Some(entries) = section(p, "Personal Property Tax") {
            let what: Vec<String> = statement_names(&entries)
                .iter()
                .map(|n| title_case(n))
                .collect();
            let listed = if what.is_empty() {
                String::new()
            } else {
                format!(" ({})", what.join(", "))
            };
            out.push(
                Group::Home,
                Kind::Document,
                format!("Personal property tax receipts{listed}"),
                "Property tax statement",
                "Personal Property Tax",
            );
        }
    }

    // Brokers.
    for (name, last4) in &brokers {
        let k = key(name);
        let paid = pages.iter().any(|p| {
            section(p, "Form 1040, Dividend Income")
                .into_iter()
                .chain(section(p, "Form 1040, Interest Income"))
                .flatten()
                .any(|l| key(&account_suffix(&strip_amounts(l.trim())).0) == k)
        });
        let what = if paid {
            "Consolidated 1099 (dividends, interest and sales)"
        } else {
            "1099-B (sales of investments)"
        };
        out.push(
            Group::Investments,
            Kind::Document,
            format!("{what} from {}{}", title_case(name), acct(last4)),
            "1099-B",
            "Broker Capital Transactions Detail Summary Report",
        );
    }

    // Forms present.
    let any = |s: &str| pages.iter().any(|p| page_has(p, s));
    if any("Social Security Worksheet") {
        out.push(
            Group::Retirement,
            Kind::Document,
            "SSA-1099 (Social Security benefits)".into(),
            "SSA-1099",
            "Social Security Worksheet",
        );
    }
    // Each form by its own printed title: the 1040 names the others in its line text ("from
    // Form 5695"), which says nothing about whether they were filed.
    if any("Profit or Loss From Business") {
        out.push(
            Group::Business,
            Kind::Document,
            format!("Your business income and expenses for {next} (Schedule C)"),
            "",
            "Schedule C",
        );
    }
    if any("Income or Loss From Rental Real Estate and Royalties") {
        out.push(
            Group::Business,
            Kind::Document,
            format!("Rental income and expenses for {next}, for each property"),
            "",
            "Schedule E, Part I",
        );
    }
    if any("Profit or Loss From Farming") {
        out.push(
            Group::Business,
            Kind::Document,
            format!("Farm income and expenses for {next} (Schedule F)"),
            "",
            "Schedule F",
        );
    }
    if any("Lifetime Learning Credits") {
        out.push(
            Group::Other,
            Kind::Document,
            "Form 1098-T for each student".into(),
            "",
            "Form 8863",
        );
    }
    if any("Premium Tax Credit (PTC)") {
        out.push(
            Group::Home,
            Kind::Document,
            "Form 1095-A (Marketplace health insurance)".into(),
            "1095",
            "Form 8962",
        );
    }
    if any("Child and Dependent Care Expenses") {
        out.push(
            Group::Other,
            Kind::Document,
            "Child and dependent care statements (provider, amount paid)".into(),
            "",
            "Form 2441",
        );
    }
    if any("Residential Energy Credits") {
        out.push(Group::Questions, Kind::Answer, format!("Did you make energy-saving improvements to your home in {next}? If so, what and how much"), "", "Form 5695");
    }
    out.push(
        Group::Questions,
        Kind::Answer,
        format!("Any changes in {next}: address, marital status, dependents, new bank or brokerage accounts?"),
        "",
        "Always asked",
    );

    let mut items = out.items;
    items.sort_by_key(|i| i.group);
    Proposal {
        year,
        taxpayer,
        spouse,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented returns laid out as pdfium reads an UltraTax CS return (no real client data).
    fn pages() -> Vec<String> {
        [
            "Filing Instructions\nIndividual Income Tax Return\nTaxable Year Ended December 31, 2025\n",
            "1040 IRA Distribution Report 2025\nT/S Payer 1099-R Box 1 1099-R Box 2a\nA\nB\nT EXAMPLE LIFE INSURANCE CO 7 5,000 5,000\nS EXAMPLE LIFE INSURANCE CO 7 2,500 2,500\n5,000 5,000\nPg 9\nAvery & Morgan Sample\nName\n###-##-0000\nTaxpayer Identification Number\n",
            "1040 Pension/Annuity Report 2025\nT/S Payer Gross Distribution Rollover Taxable Amount\nT STATE TEACHERS RETIREMENT SYS 7 18,000 18,000\nS FICTIONAL INVESTMENTS\n7\nT CITY OF EXAMPLEVILLE 7\n",
            "Form Salaries & Wages Report\nT/S Employer Federal Wages Federal Withheld Soc Sec Wages\nT SAMPLE ACCOUNTING, P.C. 21,500 2,100 9.770 24,000 1,488\n24,000 348 1,200\nMO 21,500\nS EXAMPLE R-I SCHOOL DISTRICT\nMO\nPg 12\nAvery & Morgan Sample\nName\n",
            "***-**-0000 Federal Statements Page 1\nForm 1040, Dividend Income\nPayer\nOrdinary Qualified Section 199A\nDividends Dividends Dividends\nSAMPLE FUNDS (4321)\n$ 120 $ 80 $ 5\nEXAMPLE BROKERAGE & CO INC (1234)\n90 40 10\n TOTAL\n$ 210 $ 120 $ 15\nForm 1040, Line 26 - Estimated Tax Payments and Amount Applied From Previous Year\nDescription Amount\n1ST QUARTER ESTIMATE PAYMENT 4,000\n TOTAL $ 4,000\n",
            "***-**-0000 Federal Statements Page 2\nSchedule A, Line 8a - Home Mortgage Interest & Points From Form 1098\nDescription Amount\nSAMPLE MORTGAGE LOAN $ 8,100\nFICTIONAL BANK NA 900\n TOTAL $ 9,000\n",
            "***-**-0000 Federal Statements Page 3\nStatement 1 - Schedule E, Page 2, Line 28\nName\nP For Basis Comp Not at Passive\nS Ptr EIN Required Risk Loss\nSAMPLE ACCOUNTING, P.C.\nS **-***0001 X $ $ $ $ $ 55,000\nEXAMPLE HOLDINGS, LLC\nP **-***0002\n RENTAL REAL ESTATE\nP **-***0002 6,000\n TOTAL $ 0 $ 61,000\n",
            "***-**-0000 Federal Statements Page 4\nUnit 7 Sample Point\nForm 6252 - Installment Sale History\nContract Balance Principal Reported\n2025 $ 50,000 $ 1,500\n TOTAL $ 1,500\n",
            "Federal Statements Page 1\nPersonal Property Tax\nDescription Amount\nFORD $ \nTOYOTA\n TOTAL $ 0\n",
            "Form 1040 Broker Capital Transactions Detail Summary Report 2025\nBroker Account Name Account No. T/S/J\nEXAMPLE BROKERAGE & CO INC 4400-1234 J\n1\n1\n",
            "2025 Social Security Worksheet\n1. Enter the total amount from box 5\n",
            "Form 5695 Residential Energy Credits\n",
            "Schedule 3 line 5b Energy efficient home improvement credit from Form 5695\nEducation credits from Form 8863\nExcess advance premium tax credit repayment. Attach Form 8962\n",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn prompts(p: &Proposal) -> Vec<&str> {
        p.items.iter().map(|i| i.prompt.as_str()).collect()
    }

    #[test]
    fn reads_the_supporting_reports_and_statements() {
        let p = propose(&pages());
        assert_eq!(p.year, Some(2025));
        assert_eq!(p.ask_year(), Some(2026));
        assert_eq!(
            (p.taxpayer.as_deref(), p.spouse.as_deref()),
            (Some("Avery"), Some("Morgan"))
        );
        let got = prompts(&p);
        for want in [
            "W-2 from Sample Accounting, P.C. (Avery)",
            "W-2 from Example R-I School District (Morgan)",
            "1099-R (IRA) from Example Life Insurance Co (Avery)",
            "1099-R (IRA) from Example Life Insurance Co (Morgan)",
            "1099-R (pension or annuity) from State Teachers Retirement Sys (Avery)",
            "1099-R (pension or annuity) from Fictional Investments (Morgan)",
            "1099-R (pension or annuity) from City of Exampleville (Avery)",
            "1099-DIV from Sample Funds, account ending 4321",
            "Consolidated 1099 (dividends, interest and sales) from Example Brokerage & Co Inc, account ending 1234",
            "Form 1098 (mortgage interest) from Sample Mortgage Loan",
            "Form 1098 (mortgage interest) from Fictional Bank NA",
            "K-1 from Sample Accounting, P.C. (S corporation)",
            "K-1 from Example Holdings, LLC (partnership)",
            "The dates and amounts of your 2026 federal and state estimated tax payments",
            "Payments you received in 2026 on the installment sale of Unit 7 Sample Point (principal and interest)",
            "Personal property tax receipts (Ford, Toyota)",
            "SSA-1099 (Social Security benefits)",
            "Did you make energy-saving improvements to your home in 2026? If so, what and how much",
            "Any changes in 2026: address, marital status, dependents, new bank or brokerage accounts?",
        ] {
            assert!(got.contains(&want), "missing {want:?} in {got:#?}");
        }
        // The broker's dividends are its consolidated 1099, not a second item; the rental line
        // under a K-1 entity is not an entity.
        assert!(
            !got.iter()
                .any(|g| g.starts_with("1099-DIV from Example Brokerage"))
        );
        assert!(!got.iter().any(|g| g.contains("Rental Real Estate")));
        assert_eq!(got.iter().filter(|g| g.starts_with("K-1")).count(), 2);
        // Forms only named in another form's line text are not taken as filed.
        assert!(
            !got.iter()
                .any(|g| g.contains("1098-T") || g.contains("1095-A"))
        );
        // No amounts reach an item.
        assert!(
            !got.iter()
                .any(|g| g.contains("8,100") || g.contains("5,000"))
        );
        // Doc types are Vibe Files' seeded names.
        let ty = |prefix: &str| {
            p.items
                .iter()
                .find(|i| i.prompt.starts_with(prefix))
                .map(|i| i.doc_type.as_str())
        };
        assert_eq!(ty("W-2 from"), Some("W-2"));
        assert_eq!(ty("K-1 from Sample"), Some("K-1 (1120-S)"));
        assert_eq!(ty("K-1 from Example"), Some("K-1 (1065)"));
        assert_eq!(ty("Consolidated 1099"), Some("1099-B"));
        assert!(!p.looks_empty());
    }

    #[test]
    fn a_page_that_is_not_a_return_proposes_only_the_question() {
        let p = propose(&["Invoice 1234\nThank you for your business\n".to_string()]);
        assert!(p.looks_empty());
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.year, None);
    }

    #[test]
    fn title_case_keeps_abbreviations() {
        assert_eq!(
            title_case("CHARLES SAMPLE & CO INC"),
            "Charles Sample & Co Inc"
        );
        assert_eq!(
            title_case("EXAMPLE R-I SCHOOL DISTRICT"),
            "Example R-I School District"
        );
        assert_eq!(title_case("SAMPLE BANK NA"), "Sample Bank NA");
        assert_eq!(title_case("GM"), "GM");
        assert_eq!(
            title_case("The Sample Group, P.C."),
            "The Sample Group, P.C."
        );
    }
}
