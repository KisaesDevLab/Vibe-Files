//! Routing rules: ordered, first match wins, grouped in profiles. Each profile ends with a
//! catch-all rule that cannot be deleted. Stored in `data/rules.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::clients::Client;
use crate::parse::Parsed;
use crate::template::{self, BUILTIN_TOKENS, RenderError, Tokens};

pub const RULES_FILE: &str = "rules.json";
pub const DEFAULT_PROFILE: &str = "Firm";
pub const CATCH_ALL_NAME: &str = "Catch-all";
const CATCH_ALL_PATTERN: &str = ".*";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    /// Also the `{DocType}` token.
    pub name: String,
    /// Regex tested against the filename remainder, case-insensitive.
    pub pattern: String,
    /// Folder, `/`-separated, starting with `{Client}` (the client's folder).
    pub dest_template: String,
    pub filename_template: String,
    pub requires_year: bool,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// The profile's last rule: pattern fixed to `.*`, always enabled, cannot be deleted.
    #[serde(default)]
    pub catch_all: bool,
    /// Wrap the pattern in word edges (see [`START`]/[`END`]), so `k-?1` matches "2025_K-1"
    /// but not "K-10". The pattern is still a regex.
    #[serde(default)]
    pub whole_words: bool,
    /// Real byte copies to other clients, written and verified before the primary move (P07a).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<CopyDest>,
    /// Filing into a `{Year}` folder also creates the following year's empty folder (V11, like
    /// the owner's old filer).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub next_year_folder: bool,
    /// "Except when": a regex that, when it also matches, makes the rule not match (V10; the
    /// regex crate has no negative look-ahead). Empty: no exception.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exclude: String,
}

/// One extra destination of a rule (P07a).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CopyDest {
    pub resolver: Resolver,
    /// Folder, `/`-separated, starting with `{Copy.Client}` (the copy client's folder).
    pub dest_template: String,
    pub filename_template: String,
    #[serde(default)]
    pub on_split: OnSplit,
    /// RelatedByName only: when the captured text names no client at all (not even as a
    /// suggestion), the copy is skipped with a note instead of asking for a client.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// How a copy's client is found.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resolver {
    /// A name captured by the named group: matched against the primary client's related clients
    /// first, then the whole client list (the per-shareholder K-1).
    RelatedByName { group: String },
    /// Every child of the primary client with one of these roles (empty: every child).
    Related {
        #[serde(default)]
        roles: Vec<String>,
    },
    /// A second client ID captured by the named group.
    Capture { group: String },
    /// Left blank; picked in Preview.
    Manual,
}

impl Resolver {
    pub fn label(&self) -> &'static str {
        match self {
            Resolver::RelatedByName { .. } => "Related by name · group",
            Resolver::Related { .. } => "Related · role",
            Resolver::Capture { .. } => "Capture · group",
            Resolver::Manual => "Manual · pick at review",
        }
    }
}

/// What a copy does when its file is split (P09 hook, R4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnSplit {
    /// Each piece gets its own copies.
    #[default]
    Segment,
    /// The whole original goes to the copy client once (and is kept as that copy).
    Whole,
}

impl CopyDest {
    pub fn new(resolver: Resolver) -> Self {
        Self {
            resolver,
            dest_template: "{Copy.Client}/Source Documents/{Year}".into(),
            filename_template: "{Year} {DocType} from {Client}.{Ext}".into(),
            on_split: OnSplit::Whole,
            optional: false,
        }
    }
}

fn enabled_default() -> bool {
    true
}

impl Rule {
    pub fn new(
        name: &str,
        pattern: &str,
        dest_template: &str,
        filename_template: &str,
        requires_year: bool,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            pattern: pattern.into(),
            dest_template: dest_template.into(),
            filename_template: filename_template.into(),
            requires_year,
            enabled: true,
            catch_all: false,
            whole_words: false,
            copies: Vec::new(),
            next_year_folder: false,
            exclude: String::new(),
        }
    }

    pub fn catch_all() -> Self {
        Self {
            catch_all: true,
            ..Self::new(
                CATCH_ALL_NAME,
                CATCH_ALL_PATTERN,
                "{Client}/Source Documents/{Year}",
                "{Rest}.{Ext}",
                true,
            )
        }
    }
}

/// Whole-word edges that also treat `_` as a separator (filenames use it), unlike `\b`.
pub const START: &str = "(?:^|[^a-z0-9])";
pub const END: &str = "(?:[^a-z0-9]|$)";

fn words(rule: Rule) -> Rule {
    Rule {
        whole_words: true,
        ..rule
    }
}

/// The former starter "Tax" profile (X132; replaced by [`firm_rules`] in V11), kept as the rule
/// set most tests are written against. The specific rules come
/// before Tax Return because organizers, engagement letters and K-1s usually mention a form number
/// or "return" too. K-1 package comes before K-1 (P07a); both copy the whole PDF on split (#44).
/// Writes a rules.json with only the former Tax profile, for UI tests written against it.
#[cfg(any(test, feature = "test-support"))]
pub fn seed_tax(data_dir: &Path) {
    std::fs::create_dir_all(data_dir).unwrap();
    save(
        data_dir,
        &RulesFile {
            profiles: [("Tax".to_string(), tax_rules())].into(),
            active: "Tax".into(),
        },
    )
    .unwrap();
}

#[cfg(any(test, feature = "test-support"))]
pub fn tax_rules() -> Vec<Rule> {
    vec![
        Rule {
            copies: vec![CopyDest {
                filename_template: "{Year} K-1 Package from {Client}.{Ext}".into(),
                ..CopyDest::new(Resolver::Related {
                    roles: vec!["shareholder".into(), "partner".into(), "member".into()],
                })
            }],
            ..words(Rule::new(
                "K-1 package",
                r"k-?1.*package",
                "{Client}/Tax Returns/{Year}",
                "{Year} K-1 Package - {Client}.{Ext}",
                true,
            ))
        },
        Rule {
            copies: vec![CopyDest {
                filename_template: "{Year} K-1 from {Client}.{Ext}".into(),
                ..CopyDest::new(Resolver::RelatedByName {
                    group: "who".into(),
                })
            }],
            ..words(Rule::new(
                "K-1",
                r"k-?1\s+(?P<who>.+)",
                "{Client}/Tax Returns/{Year}",
                "{Year} K-1 - {Copy.Client}.{Ext}",
                true,
            ))
        },
        words(Rule::new(
            "Organizer",
            "organizer",
            "{Client}/Source Documents/{Year}",
            "{Year} Organizer - {Client}.{Ext}",
            true,
        )),
        words(Rule::new(
            "Engagement Letter",
            "engagement|el",
            "{Client}/Engagement Letters",
            "{Year} Engagement Letter - {Client}.{Ext}",
            true,
        )),
        Rule::new(
            "Tax Return",
            // Form numbers with an optional -X/-S/... suffix, but not 1040-ES (estimate
            // vouchers): the character after the form may not be '-' unless it starts a known
            // suffix. Written without look-ahead, which the regex crate lacks.
            &format!(
                "{START}(?:1040|1120|1065|1041|990)(?:-?(?:x|s|sr|nr|pf|ez|t))?(?:[^a-z0-9-]|$)|{START}return{END}"
            ),
            "{Client}/Tax Returns/{Year}",
            "{Year} {DocType} - {Client}.{Ext}",
            true,
        ),
        Rule::catch_all(),
    ]
}

/// The owner's old filer's rules (V11), reviewed 2026-11-02: (name, pattern, folder below the
/// client, group by year). Patterns are whole words, any case; `[\s_-]+` lets a space also be
/// `_` or `-`, `s?` takes singular and plural, and a trailing `\d*` lets a number follow right
/// after a word that ends in a letter ("UltraTax2025"; never after a form number, so "K-10" and
/// "W-22" don't match K-1 / W-2). The more specific rules come before the general ones
/// (Setup of Entity / Stock Basis before Permanent, Sales Tax before Tax Documents, CS Accounting
/// Products before CSAccounting); the rules added at the review (Tax Forms, Bank Statements,
/// Engagement Letter, Notices and Letters) sit where they don't take files from the old ones.
const FIRM_RULES: [(&str, &str, &str, bool); 28] = [
    ("UltraTax", r"ultra[\s_-]*tax\d*", "Income Tax", true),
    (
        "Fixed Assets",
        r"fixed[\s_-]+assets?\d*",
        "Income Tax",
        true,
    ),
    (
        "CS Accounting Products",
        r"cs[\s_-]*accounting[\s_-]+products?\d*",
        "Bookkeeping",
        true,
    ),
    (
        "CSAccounting",
        r"cs[\s_-]*accounting\d*",
        "Bookkeeping",
        true,
    ),
    (
        "Practice CS",
        r"practice[\s_-]+cs\d*",
        "Other/Practice",
        true,
    ),
    ("Payroll", r"payroll\d*", "Payroll", true),
    (
        "Amortization Schedules",
        r"amortization[\s_-]+schedules?\d*",
        "Workpapers & Support",
        false,
    ),
    (
        "Engagement CS",
        r"engagement[\s_-]+cs\d*",
        "Financials",
        true,
    ),
    (
        "IRS Correspondence",
        r"irs[\s_-]+correspondence\d*",
        "Correspondence",
        true,
    ),
    ("Sales Tax", r"sales[\s_-]+tax\d*", "Sales Tax", true),
    (
        "Tax Documents",
        r"tax[\s_-]+documents?\d*",
        "Income Tax",
        true,
    ),
    (
        "Financials",
        r"financials?(?:[\s_-]+statements?)?\d*",
        "Financials",
        true,
    ),
    (
        "General Ledger",
        r"general[\s_-]+ledgers?\d*",
        "Bookkeeping",
        true,
    ),
    (
        "Vendor Documents",
        r"vendor[\s_-]+documents?\d*",
        "Bookkeeping",
        true,
    ),
    (
        "Setup of Entity",
        r"setup[\s_-]+of[\s_-]+entity\d*",
        "Permanent/Setup",
        false,
    ),
    (
        "Stock Basis Information",
        r"stock[\s_-]+basis(?:[\s_-]+information)?\d*",
        "Permanent/Stock Basis",
        false,
    ),
    ("Permanent", r"permanent\d*", "Permanent", false),
    (
        "State Correspondence",
        r"state[\s_-]+correspondence\d*",
        "Correspondence",
        true,
    ),
    (
        "Audit Reports",
        r"audit[\s_-]+reports?\d*",
        "Financials",
        true,
    ),
    (
        "Client Agreement",
        r"client[\s_-]+agreements?\d*",
        "Other",
        true,
    ),
    (
        "Correspondence",
        r"correspondence\d*",
        "Correspondence",
        true,
    ),
    (
        "Tax Forms",
        r"w-?2|1099(?:-?[a-z]+)?|1098(?:-?[a-z]+)?|organizers?\d*",
        "Income Tax",
        true,
    ),
    (
        "Bank Statements",
        r"bank[\s_-]+statements?\d*",
        "Bookkeeping",
        true,
    ),
    (
        "Engagement Letter",
        r"engagement[\s_-]+letters?\d*",
        "Other",
        true,
    ),
    ("Documents", r"documents?\d*", "Other", true),
    (
        "Tax Returns",
        r"tax[\s_-]+returns?\d*",
        "Workpapers & Support",
        true,
    ),
    (
        "Notices and Letters",
        r"(?:irs[\s_-]+|state[\s_-]+)?(?:notices?|letters?)\d*",
        "Correspondence",
        true,
    ),
    ("K-1 other", r"k-?1s?", "Income Tax", true),
];

/// The default "Firm" profile (V11): the K-1 rules with their copies, the old filer's rules
/// (full original name kept; year folders also get next year's), then Catch-all to Not Yet Filed.
pub fn firm_rules() -> Vec<Rule> {
    let k1_copy = |name: &str, resolver| CopyDest {
        dest_template: "{Copy.Client}/Income Tax/{Year}".into(),
        filename_template: name.into(),
        ..CopyDest::new(resolver)
    };
    let mut out = vec![
        Rule {
            copies: vec![k1_copy(
                "{Year} K-1 Package from {Client}.{Ext}",
                Resolver::Related {
                    roles: vec!["shareholder".into(), "partner".into(), "member".into()],
                },
            )],
            ..words(Rule::new(
                "K-1 package",
                r"k-?1.*package",
                "{Client}/Income Tax/{Year}",
                "{Year} K-1 Package - {Client}.{Ext}",
                true,
            ))
        },
        // The text after "K-1" is a partner only when it names a client (a related one, or one
        // in the list, at least as a suggestion); otherwise the K-1 is filed without a copy.
        Rule {
            copies: vec![CopyDest {
                optional: true,
                ..k1_copy(
                    "{Year} K-1 from {Client}.{Ext}",
                    Resolver::RelatedByName {
                        group: "who".into(),
                    },
                )
            }],
            ..words(Rule::new(
                "K-1",
                r"k-?1\s+(?P<who>.+)",
                "{Client}/Income Tax/{Year}",
                "{Year} K-1 - {Copy.Client}.{Ext}",
                true,
            ))
        },
    ];
    for (name, pattern, path, by_year) in FIRM_RULES {
        let dest = if by_year {
            format!("{{Client}}/{path}/{{Year}}")
        } else {
            format!("{{Client}}/{path}")
        };
        out.push(Rule {
            next_year_folder: by_year,
            ..words(Rule::new(name, pattern, &dest, "{Original}.{Ext}", by_year))
        });
    }
    out.push(Rule {
        catch_all: true,
        ..Rule::new(
            CATCH_ALL_NAME,
            CATCH_ALL_PATTERN,
            "{Client}/Not Yet Filed",
            "{Original}.{Ext}",
            false,
        )
    });
    out
}

/// All profiles plus which one is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesFile {
    pub profiles: BTreeMap<String, Vec<Rule>>,
    pub active: String,
}

impl Default for RulesFile {
    fn default() -> Self {
        Self {
            profiles: [(DEFAULT_PROFILE.to_string(), firm_rules())].into(),
            active: DEFAULT_PROFILE.to_string(),
        }
    }
}

impl RulesFile {
    /// Repairs structure: at least one profile, a valid active profile, and exactly one
    /// catch-all at the end of each profile (pattern `.*`, enabled).
    pub fn normalize(&mut self) {
        if self.profiles.is_empty() {
            *self = Self::default();
            return;
        }
        if !self.profiles.contains_key(&self.active) {
            self.active = self.profiles.keys().next().cloned().unwrap_or_default();
        }
        for rules in self.profiles.values_mut() {
            let last_catch_all = rules.iter().rposition(|r| r.catch_all);
            let mut catch_all = match last_catch_all {
                Some(i) => rules.remove(i),
                None => Rule::catch_all(),
            };
            rules.retain(|r| !r.catch_all);
            catch_all.pattern = CATCH_ALL_PATTERN.into();
            catch_all.enabled = true;
            rules.push(catch_all);
        }
    }

    pub fn active_rules(&self) -> &[Rule] {
        self.profiles
            .get(&self.active)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn active_rules_mut(&mut self) -> &mut Vec<Rule> {
        self.profiles.entry(self.active.clone()).or_default()
    }
}

/// Moves the rule at `from` to position `to`, never past the catch-all (which stays last).
pub fn move_rule(rules: &mut Vec<Rule>, from: usize, to: usize) {
    let last = rules.len().saturating_sub(1);
    if from >= last || rules[from].catch_all {
        return;
    }
    let to = to.min(last.saturating_sub(1));
    let rule = rules.remove(from);
    rules.insert(to, rule);
}

/// A new rule inserted just before the catch-all. Returns its index.
pub fn add_rule(rules: &mut Vec<Rule>) -> usize {
    let at = rules.len().saturating_sub(1);
    rules.insert(
        at,
        Rule::new(
            "New rule",
            "",
            "{Client}/Source Documents/{Year}",
            "{Rest}.{Ext}",
            true,
        ),
    );
    at
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(RULES_FILE)
}

/// Loads `rules.json`, writing the default "Tax" profile on first run. A corrupt file gives the
/// default and `false` (do not overwrite it), plus the error for the Rules tab to show.
pub fn load_or_seed(data_dir: &Path) -> (RulesFile, bool, Option<String>) {
    let p = path(data_dir);
    match std::fs::read_to_string(&p) {
        Ok(text) => match serde_json::from_str::<RulesFile>(&text) {
            Ok(mut rules) => {
                rules.normalize();
                (rules, true, None)
            }
            Err(e) => {
                tracing::error!(error = %e, "rules.json is invalid; not saving over it");
                (
                    RulesFile::default(),
                    false,
                    Some(format!(
                        "rules.json could not be read ({e}); showing the default rules."
                    )),
                )
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let rules = RulesFile::default();
            if let Err(e) = save(data_dir, &rules) {
                tracing::error!(error = %e, "seeding rules.json failed");
            }
            (rules, true, None)
        }
        Err(e) => {
            tracing::error!(error = %e, "cannot read rules.json");
            (
                RulesFile::default(),
                false,
                Some(format!(
                    "rules.json could not be read ({e}); showing the default rules."
                )),
            )
        }
    }
}

pub fn save(data_dir: &Path, rules: &RulesFile) -> anyhow::Result<()> {
    let p = path(data_dir);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(rules)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation

pub fn compile_pattern(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern).case_insensitive(true).build()
}

/// The regex a rule actually runs: its pattern, wrapped in word edges when `whole_words` is on.
pub fn effective_pattern(rule: &Rule) -> String {
    if rule.whole_words && !rule.catch_all {
        format!("{START}(?:{}){END}", rule.pattern)
    } else {
        rule.pattern.clone()
    }
}

/// `Copy.Client`, `Copy.ID`, `Copy.Role` and `Copy.<column>` for each CSV extra token.
pub fn copy_tokens(extra_tokens: &[String]) -> Vec<String> {
    let mut out: Vec<String> = ["Copy.Client", "Copy.ID", "Copy.Role"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.extend(extra_tokens.iter().map(|t| format!("Copy.{t}")));
    out
}

/// Problems with one rule. `extra_tokens` are the CSV extra-column tokens.
pub fn validate_rule(rule: &Rule, extra_tokens: &[String]) -> Vec<String> {
    let copy_tokens = copy_tokens(extra_tokens);
    let mut primary_allowed: Vec<&str> = BUILTIN_TOKENS.to_vec();
    primary_allowed.extend(extra_tokens.iter().map(String::as_str));
    let mut copy_allowed = primary_allowed.clone();
    copy_allowed.extend(copy_tokens.iter().map(String::as_str));
    // A primary template may use {Copy.*} only when the rule has exactly one copy.
    let allowed: Vec<&str> = if rule.copies.len() == 1 {
        copy_allowed.clone()
    } else {
        primary_allowed.clone()
    };
    let mut errors = Vec::new();

    if rule.name.trim().is_empty() {
        errors.push("The rule name is empty.".to_string());
    }
    if !rule.catch_all {
        if rule.pattern.trim().is_empty() {
            errors.push("The pattern is empty.".to_string());
        } else if let Err(e) = compile_pattern(&effective_pattern(rule)) {
            errors.push(format!("The pattern is not a valid regex: {e}"));
        }
        if !rule.exclude.trim().is_empty()
            && let Err(e) = compile_pattern(&rule.exclude)
        {
            errors.push(format!("\"Except when\" is not a valid regex: {e}"));
        }
    }

    let mut segments = rule.dest_template.split('/');
    if segments.next().map(str::trim) != Some("{Client}") {
        errors.push(
            "The destination must start with {Client} (the client's folder), e.g. {Client}/Tax Returns/{Year}."
                .to_string(),
        );
    }
    for seg in segments {
        errors.extend(
            template::validate_segment(seg, &allowed)
                .into_iter()
                .map(|e| format!("Destination: {e}")),
        );
    }

    if rule.filename_template.contains('/') || rule.filename_template.contains('\\') {
        errors.push("The file name cannot contain / or \\.".to_string());
    } else {
        errors.extend(
            template::validate_segment(&rule.filename_template, &allowed)
                .into_iter()
                .map(|e| format!("File name: {e}")),
        );
    }
    let filename_tokens = template::tokens_in(&rule.filename_template).unwrap_or_default();
    if !filename_tokens.iter().any(|t| t == "Ext") {
        errors.push(
            "The file name must keep the extension: include {Ext}, e.g. {Rest}.{Ext}.".into(),
        );
    }

    let compiled = compile_pattern(&effective_pattern(rule)).ok();
    for (n, copy) in rule.copies.iter().enumerate() {
        let what = format!("Copy {}", n + 1);
        let mut segments = copy.dest_template.split('/');
        if segments.next().map(str::trim) != Some("{Copy.Client}") {
            errors.push(format!(
                "{what}: the destination must start with {{Copy.Client}} (the copy client's folder)."
            ));
        }
        for seg in segments {
            errors.extend(
                template::validate_segment(seg, &copy_allowed)
                    .into_iter()
                    .map(|e| format!("{what} destination: {e}")),
            );
        }
        if copy.filename_template.contains('/') || copy.filename_template.contains('\\') {
            errors.push(format!("{what}: the file name cannot contain / or \\."));
        } else {
            errors.extend(
                template::validate_segment(&copy.filename_template, &copy_allowed)
                    .into_iter()
                    .map(|e| format!("{what} file name: {e}")),
            );
            if !template::tokens_in(&copy.filename_template)
                .unwrap_or_default()
                .iter()
                .any(|t| t == "Ext")
            {
                errors.push(format!(
                    "{what}: the file name must keep the extension ({{Ext}})."
                ));
            }
        }
        if let Resolver::RelatedByName { group } | Resolver::Capture { group } = &copy.resolver {
            let has_group = compiled
                .as_ref()
                .is_some_and(|re| re.capture_names().flatten().any(|g| g == group));
            if group.trim().is_empty() {
                errors.push(format!("{what}: name the capture group."));
            } else if !has_group {
                errors.push(format!("{what}: the pattern has no (?P<{group}>…) group."));
            }
        }
    }

    let mut templates = vec![&rule.dest_template, &rule.filename_template];
    for c in &rule.copies {
        templates.push(&c.dest_template);
        templates.push(&c.filename_template);
    }
    let uses_year = templates.iter().any(|t| {
        template::tokens_in(t)
            .unwrap_or_default()
            .iter()
            .any(|x| x == "Year")
    });
    if uses_year && !rule.requires_year {
        errors.push("The rule uses {Year}, so \"Requires year\" must be on.".to_string());
    }
    errors
}

/// Problems with a profile's rules, as (rule index, message).
pub fn validate_profile(rules: &[Rule], extra_tokens: &[String]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        for e in validate_rule(r, extra_tokens) {
            out.push((i, e));
        }
        let dup = rules[..i]
            .iter()
            .any(|o| o.name.trim().eq_ignore_ascii_case(r.name.trim()));
        if dup && !r.name.trim().is_empty() {
            out.push((i, format!("Another rule is already named \"{}\".", r.name)));
        }
    }
    out
}

/// Problems with the whole file: every profile's rules, and profile names.
pub fn validate_file(file: &RulesFile, extra_tokens: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for (name, rules) in &file.profiles {
        if name.trim().is_empty() {
            out.push("A profile has an empty name.".to_string());
        }
        for (i, e) in validate_profile(rules, extra_tokens) {
            out.push(format!("{name} / {}: {e}", rules[i].name));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Matching and evaluation

/// The enabled rules of a profile with their regexes compiled.
pub struct CompiledRules {
    rules: Vec<(Rule, Regex)>,
    /// "Except when" patterns, by rule id.
    excludes: std::collections::HashMap<String, Regex>,
}

impl CompiledRules {
    /// Fails if any enabled rule's pattern doesn't compile (validation catches this first).
    pub fn new(rules: &[Rule]) -> Result<Self, String> {
        let mut out = Vec::new();
        let mut excludes = std::collections::HashMap::new();
        for r in rules.iter().filter(|r| r.enabled || r.catch_all) {
            let re = compile_pattern(&effective_pattern(r))
                .map_err(|e| format!("rule \"{}\": {e}", r.name))?;
            if !r.catch_all && !r.exclude.trim().is_empty() {
                let ex = compile_pattern(&r.exclude)
                    .map_err(|e| format!("rule \"{}\" (except when): {e}", r.name))?;
                excludes.insert(r.id.clone(), ex);
            }
            out.push((r.clone(), re));
        }
        if !out.iter().any(|(r, _)| r.catch_all) {
            out.push((
                Rule::catch_all(),
                compile_pattern(CATCH_ALL_PATTERN).expect("valid"),
            ));
        }
        Ok(Self {
            rules: out,
            excludes,
        })
    }

    /// First enabled rule whose pattern matches `rest`; the catch-all if none does.
    pub fn first_match(&self, rest: &str) -> &Rule {
        self.first_specific_match(rest).unwrap_or_else(|| {
            &self
                .rules
                .iter()
                .find(|(r, _)| r.catch_all)
                .expect("catch-all always present")
                .0
        })
    }

    /// The named groups `rule_id`'s pattern captures in `text` (P07a copies), by name.
    pub fn captures(&self, rule_id: &str, text: &str) -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        if let Some((_, re)) = self.rules.iter().find(|(r, _)| r.id == rule_id)
            && let Some(c) = re.captures(text)
        {
            for name in re.capture_names().flatten() {
                if let Some(m) = c.name(name) {
                    out.insert(name.to_string(), m.as_str().trim().to_string());
                }
            }
        }
        out
    }

    /// First enabled rule other than the catch-all whose pattern matches `text`. Used for
    /// filenames and, in P06, for first-page text.
    pub fn first_specific_match(&self, text: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|(r, re)| {
                !r.catch_all
                    && re.is_match(text)
                    && !self.excludes.get(&r.id).is_some_and(|ex| ex.is_match(text))
            })
            .map(|(r, _)| r)
    }
}

/// Where a rule sends a file, inside the client's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Folders below the client folder (empty for `{Client}` alone).
    pub dest_rel: PathBuf,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitProblem {
    /// The rule needs a year and none was found or given.
    NeedsYear,
    Render(RenderError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleHit {
    pub rule_id: String,
    pub rule_name: String,
    pub outcome: Result<Rendered, HitProblem>,
}

impl RuleHit {
    #[cfg_attr(not(test), allow(dead_code))] // used by the plan builder (P06)
    pub fn needs_year(&self) -> bool {
        self.outcome == Err(HitProblem::NeedsYear)
    }
}

/// Fills `rule`'s templates for this file and client. `year` is the file's year or the batch
/// year chosen in Preview.
pub fn evaluate_rule(rule: &Rule, parsed: &Parsed, client: &Client, year: Option<u16>) -> RuleHit {
    evaluate_rule_with_copy(rule, parsed, client, year, None)
}

/// The `{Copy.*}` tokens for `copy` (client and role), or their fallbacks when the copy client is
/// not known yet: `{Copy.Client}` becomes the remainder, the others empty (P07a).
pub fn copy_token_values(
    copy: Option<(&Client, &str)>,
    parsed: &Parsed,
    extra_tokens: impl Iterator<Item = String>,
) -> Tokens {
    let mut t = Tokens::new();
    match copy {
        Some((c, role)) => {
            t.insert("Copy.Client".into(), c.name.clone());
            t.insert("Copy.ID".into(), c.id.clone());
            t.insert("Copy.Role".into(), role.to_string());
            for (k, v) in &c.extra {
                t.insert(format!("Copy.{k}"), v.clone());
            }
            for k in extra_tokens {
                t.entry(format!("Copy.{k}")).or_default();
            }
        }
        None => {
            t.insert("Copy.Client".into(), parsed.rest.clone());
            t.insert("Copy.ID".into(), String::new());
            t.insert("Copy.Role".into(), String::new());
            for k in extra_tokens {
                t.insert(format!("Copy.{k}"), String::new());
            }
        }
    }
    t
}

/// Like [`evaluate_rule`], with the copy client the primary's `{Copy.*}` tokens refer to (a rule
/// with exactly one copy).
pub fn evaluate_rule_with_copy(
    rule: &Rule,
    parsed: &Parsed,
    client: &Client,
    year: Option<u16>,
    copy: Option<(&Client, &str)>,
) -> RuleHit {
    let hit = |outcome| RuleHit {
        rule_id: rule.id.clone(),
        rule_name: rule.name.clone(),
        outcome,
    };
    if rule.requires_year && year.is_none() {
        return hit(Err(HitProblem::NeedsYear));
    }
    let mut tokens: Tokens = template::client_tokens(client);
    if let Some(y) = year {
        tokens.insert("Year".into(), y.to_string());
    }
    tokens.insert("DocType".into(), rule.name.clone());
    tokens.insert("Rest".into(), parsed.rest.clone());
    tokens.insert("Original".into(), parsed.stem.clone());
    tokens.insert("Ext".into(), parsed.ext.clone());
    if rule.copies.len() == 1 {
        tokens.extend(copy_token_values(
            copy,
            parsed,
            client.extra.keys().cloned(),
        ));
    }

    let render = || -> Result<Rendered, RenderError> {
        let mut dest_rel = PathBuf::new();
        for seg in rule.dest_template.split('/').skip(1) {
            dest_rel.push(template::render_segment(seg, &tokens)?);
        }
        let filename = template::render_segment(&rule.filename_template, &tokens)?;
        Ok(Rendered { dest_rel, filename })
    };
    hit(render().map_err(HitProblem::Render))
}

/// Where a copy of `rule` goes for `copy_client` (P07a): the copy's templates with the primary
/// client's tokens plus `{Copy.*}`. The folder part is below the copy client's folder.
pub fn evaluate_copy(
    rule: &Rule,
    copy: &CopyDest,
    parsed: &Parsed,
    client: &Client,
    year: Option<u16>,
    copy_client: &Client,
    role: &str,
) -> Result<Rendered, HitProblem> {
    if rule.requires_year && year.is_none() {
        return Err(HitProblem::NeedsYear);
    }
    let mut tokens: Tokens = template::client_tokens(client);
    if let Some(y) = year {
        tokens.insert("Year".into(), y.to_string());
    }
    tokens.insert("DocType".into(), rule.name.clone());
    tokens.insert("Rest".into(), parsed.rest.clone());
    tokens.insert("Original".into(), parsed.stem.clone());
    tokens.insert("Ext".into(), parsed.ext.clone());
    tokens.extend(copy_token_values(
        Some((copy_client, role)),
        parsed,
        client.extra.keys().cloned(),
    ));
    let render = || -> Result<Rendered, RenderError> {
        let mut dest_rel = PathBuf::new();
        for seg in copy.dest_template.split('/').skip(1) {
            dest_rel.push(template::render_segment(seg, &tokens)?);
        }
        let filename = template::render_segment(&copy.filename_template, &tokens)?;
        Ok(Rendered { dest_rel, filename })
    };
    render().map_err(HitProblem::Render)
}

/// First-match rule for `parsed.rest`, evaluated.
#[cfg_attr(not(test), allow(dead_code))] // used by the plan builder (P06)
pub fn evaluate(
    rules: &CompiledRules,
    parsed: &Parsed,
    client: &Client,
    year: Option<u16>,
) -> RuleHit {
    evaluate_rule(rules.first_match(&parsed.rest), parsed, client, year)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{DEFAULT_ID_PATTERN, filename_regex, parse_filename};

    fn client() -> Client {
        Client {
            uuid: uuid::Uuid::new_v4(),
            id: "1234".into(),
            name: "Smith, John".into(),
            extra: [("Partner".to_string(), "KW".to_string())].into(),
        }
    }

    fn parsed(file: &str) -> Parsed {
        parse_filename(file, &filename_regex(DEFAULT_ID_PATTERN).unwrap(), 2026)
    }

    fn matched(rules: &CompiledRules, rest: &str) -> String {
        rules.first_match(rest).name.clone()
    }

    #[test]
    fn starter_rules_are_valid_and_end_with_catch_all() {
        let rules = tax_rules();
        assert!(
            validate_profile(&rules, &[]).is_empty(),
            "{:?}",
            validate_profile(&rules, &[])
        );
        assert!(rules.last().unwrap().catch_all);
    }

    /// V11: the Firm profile is valid, keeps the old filer's order, and matches whole words.
    #[test]
    fn firm_rules_match_like_the_old_filer() {
        let rules = firm_rules();
        assert!(
            validate_profile(&rules, &[]).is_empty(),
            "{:?}",
            validate_profile(&rules, &[])
        );
        assert_eq!(rules.len(), 31);
        assert!(rules.last().unwrap().catch_all);
        assert_eq!(RulesFile::default().active, "Firm");
        let r = CompiledRules::new(&rules).unwrap();
        for (rest, want) in [
            ("UltraTax 2025 1040", "UltraTax"),
            ("2025 ultratax", "UltraTax"),
            ("Fixed_Assets 2025", "Fixed Assets"),
            ("Fixed-Assets", "Fixed Assets"),
            ("2025 K-1 Baker, Thomas", "K-1"),
            ("2025 K-1 package", "K-1 package"),
            ("Tax Documents 2025", "Tax Documents"),
            ("Other Documents", "Documents"),
            ("IRS Correspondence 2024", "IRS Correspondence"),
            ("Correspondence", "Correspondence"),
            ("Payroll Q1 2025", "Payroll"),
            ("Payrolls 2025", "Catch-all"),
            ("Setup of Entity", "Setup of Entity"),
            ("Tax Returns 2025", "Tax Returns"),
            ("random scan", "Catch-all"),
            // The 2026-11-02 review: singular / plural, spacing, a number right after a word,
            // specific before general, and the added rules.
            ("2025 Tax Return", "Tax Returns"),
            ("Audit Report 2025", "Audit Reports"),
            ("Financial Statements 2025", "Financials"),
            ("Fixed Asset 2025", "Fixed Assets"),
            ("Vendor Document", "Vendor Documents"),
            ("2025 Amortization Schedule", "Amortization Schedules"),
            ("CS Accounting 2025", "CSAccounting"),
            ("CS Accounting Products 2025", "CS Accounting Products"),
            ("Ultra Tax 2025", "UltraTax"),
            ("UltraTax2025", "UltraTax"),
            ("Permanent Setup of Entity", "Setup of Entity"),
            (
                "Permanent Stock Basis Information",
                "Stock Basis Information",
            ),
            ("Stock Basis 2025", "Stock Basis Information"),
            ("Sales Tax Documents 2025", "Sales Tax"),
            ("W-2 2025", "Tax Forms"),
            ("1099-INT 2025", "Tax Forms"),
            ("2025 Organizer", "Tax Forms"),
            ("Vendor Documents 1099", "Vendor Documents"),
            ("Bank statement Jan 2025", "Bank Statements"),
            ("Engagement Letter 2025", "Engagement Letter"),
            ("IRS Notice 2025", "Notices and Letters"),
            ("Tax Return letter 2025", "Tax Returns"),
            ("2025 K-1", "K-1 other"),
            ("K-10 2025", "Catch-all"),
            ("W-22 2025", "Catch-all"),
            ("10990 2025", "Catch-all"),
        ] {
            assert_eq!(matched(&r, rest), want, "{rest}");
        }
        let ultra = rules.iter().find(|x| x.name == "UltraTax").unwrap();
        assert_eq!(ultra.dest_template, "{Client}/Income Tax/{Year}");
        assert!(ultra.requires_year && ultra.next_year_folder);
        let setup = rules.iter().find(|x| x.name == "Setup of Entity").unwrap();
        assert_eq!(setup.dest_template, "{Client}/Permanent/Setup");
        assert!(!setup.requires_year && !setup.next_year_folder);
        let irs = rules
            .iter()
            .find(|x| x.name == "IRS Correspondence")
            .unwrap();
        assert_eq!(irs.dest_template, "{Client}/Correspondence/{Year}");
        // The full original name is kept.
        let hit = evaluate_rule(
            ultra,
            &parsed("Smith, John_1234_UltraTax 2025.pdf"),
            &client(),
            Some(2025),
        );
        let out = hit.outcome.unwrap();
        assert_eq!(out.filename, "Smith, John_1234_UltraTax 2025.pdf");
        assert_eq!(out.dest_rel, PathBuf::from("Income Tax").join("2025"));
        let catch = rules.last().unwrap();
        let out = evaluate_rule(catch, &parsed("Smith, John_1234_misc.pdf"), &client(), None)
            .outcome
            .unwrap();
        assert_eq!(out.dest_rel, PathBuf::from("Not Yet Filed"));
    }

    /// V10: "Except when" stops a rule from matching.
    #[test]
    fn exclude_pattern_skips_the_rule() {
        let mut w2 = Rule::new("W-2", "w-2", "{Client}", "{Rest}.{Ext}", false);
        w2.exclude = "draft".into();
        let rules = vec![w2, Rule::catch_all()];
        assert!(validate_profile(&rules, &[]).is_empty());
        let r = CompiledRules::new(&rules).unwrap();
        assert_eq!(matched(&r, "2025 W-2"), "W-2");
        assert_eq!(matched(&r, "2025 W-2 DRAFT"), "Catch-all");
        let mut bad = rules.clone();
        bad[0].exclude = "(".into();
        assert!(!validate_profile(&bad, &[]).is_empty());
    }

    #[test]
    fn starter_patterns_match_the_right_documents() {
        let r = CompiledRules::new(&tax_rules()).unwrap();
        for rest in [
            "2025 1040",
            "2025_1040",
            "1040-X amended",
            "1040X",
            "2025 1120-S",
            "1120S",
            "1065",
            "1041 trust",
            "990-PF",
            "2025 return",
            "Return 2025",
        ] {
            assert_eq!(matched(&r, rest), "Tax Return", "{rest}");
        }
        for rest in ["2025 1040-ES vouchers", "1040ES", "returned mail", "10401"] {
            assert_ne!(matched(&r, rest), "Tax Return", "{rest}");
        }
        for rest in ["2025 Organizer", "organizer_2025", "2025 1040 organizer"] {
            assert_eq!(matched(&r, rest), "Organizer", "{rest}");
        }
        for rest in [
            "2025 EL",
            "EL_2025",
            "engagement letter 2025",
            "2025 1040 engagement",
        ] {
            assert_eq!(matched(&r, rest), "Engagement Letter", "{rest}");
        }
        for rest in ["Fidelity 2025", "Elk Grove lease", "Delta"] {
            assert_eq!(matched(&r, rest), CATCH_ALL_NAME, "{rest}");
        }
        // K-1 needs the shareholder's name after it (P07a #50); K-1 package comes first.
        for rest in [
            "2025 K-1 ABC Partners",
            "K-1 Baker, Thomas",
            "2025 k-1 from 1065",
        ] {
            assert_eq!(matched(&r, rest), "K-1", "{rest}");
        }
        for rest in ["K-1 package 2025", "2025_K1 package"] {
            assert_eq!(matched(&r, rest), "K-1 package", "{rest}");
        }
        assert_eq!(
            matched(&r, "K1_2025"),
            CATCH_ALL_NAME,
            "no shareholder name"
        );
        assert_eq!(matched(&r, "2025 bank statement"), CATCH_ALL_NAME);
    }

    #[test]
    fn copies_are_validated() {
        let base = || {
            Rule::new(
                "K-1",
                r"k-?1\s+(?P<who>.+)",
                "{Client}/Tax Returns/{Year}",
                "{Year} K-1 - {Copy.Client}.{Ext}",
                true,
            )
        };
        // {Copy.*} in the primary needs exactly one copy.
        let no_copy = base();
        assert!(
            validate_rule(&no_copy, &[])
                .iter()
                .any(|e| e.contains("Copy.Client"))
        );
        let mut one = base();
        one.copies.push(CopyDest::new(Resolver::RelatedByName {
            group: "who".into(),
        }));
        assert_eq!(validate_rule(&one, &[]), Vec::<String>::new());
        // The seeded K-1 rules are valid as shipped.
        for r in tax_rules() {
            assert_eq!(validate_rule(&r, &[]), Vec::<String>::new(), "{}", r.name);
        }
        // Group must exist; destination must start with {Copy.Client}; CSV tokens as Copy.<col>.
        let mut bad = base();
        bad.copies.push(CopyDest {
            dest_template: "{Client}/x".into(),
            filename_template: "{Copy.Partner} {Rest}.{Ext}".into(),
            ..CopyDest::new(Resolver::Capture {
                group: "id2".into(),
            })
        });
        let errors = validate_rule(&bad, &[]);
        assert!(errors.iter().any(|e| e.contains("(?P<id2>")), "{errors:?}");
        assert!(
            errors.iter().any(|e| e.contains("{Copy.Client} (the copy")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("Copy.Partner")),
            "{errors:?}"
        );
        let errors = validate_rule(&bad, &["Partner".into()]);
        assert!(
            !errors.iter().any(|e| e.contains("Copy.Partner")),
            "{errors:?}"
        );
        // Two copies: {Copy.Client} in the primary is unknown again.
        let mut two = one.clone();
        two.copies.push(CopyDest::new(Resolver::Manual));
        assert!(
            validate_rule(&two, &[])
                .iter()
                .any(|e| e.contains("Copy.Client"))
        );
    }

    #[test]
    fn unresolved_copy_client_falls_back_to_rest() {
        let mut rule = Rule::new(
            "K-1",
            r"k-?1\s+(?P<who>.+)",
            "{Client}/Tax Returns/{Year}",
            "{Year} K-1 - {Copy.Client}.{Ext}",
            true,
        );
        rule.copies.push(CopyDest::new(Resolver::RelatedByName {
            group: "who".into(),
        }));
        let re = filename_regex(DEFAULT_ID_PATTERN).unwrap();
        let parsed = parse_filename("Riverbend_4472_K-1 Baker, Thomas.pdf", &re, 2026);
        let hit = evaluate_rule(&rule, &parsed, &client(), Some(2025));
        assert_eq!(
            hit.outcome.unwrap().filename,
            "2025 K-1 - K-1 Baker, Thomas.pdf"
        );
        let baker = Client {
            uuid: uuid::Uuid::new_v4(),
            id: "2093".into(),
            name: "Baker, Thomas".into(),
            extra: Default::default(),
        };
        let hit = evaluate_rule_with_copy(
            &rule,
            &parsed,
            &client(),
            Some(2025),
            Some((&baker, "partner")),
        );
        assert_eq!(
            hit.outcome.unwrap().filename,
            "2025 K-1 - Baker, Thomas.pdf"
        );
    }

    #[test]
    fn whole_words_switch_adds_edges() {
        let mut rule = Rule::new("K", "k-?1", "{Client}", "{Rest}.{Ext}", false);
        let plain = CompiledRules::new(&[rule.clone(), Rule::catch_all()]).unwrap();
        assert_eq!(
            matched(&plain, "K-10 notes"),
            "K",
            "plain regex matches inside words"
        );
        rule.whole_words = true;
        let words = CompiledRules::new(&[rule.clone(), Rule::catch_all()]).unwrap();
        assert_eq!(matched(&words, "K-10 notes"), CATCH_ALL_NAME);
        for rest in ["2025_K-1", "k1 2025", "K-1", "x (k-1)"] {
            assert_eq!(matched(&words, rest), "K", "{rest}");
        }
        // Alternatives get the edges as a group.
        rule.pattern = "engagement|el".into();
        let words = CompiledRules::new(&[rule.clone(), Rule::catch_all()]).unwrap();
        assert_eq!(matched(&words, "2025 EL"), "K");
        assert_eq!(matched(&words, "Fidelity"), CATCH_ALL_NAME);
        assert_eq!(
            effective_pattern(&rule),
            format!("{START}(?:engagement|el){END}")
        );
        // Old rules.json without the field load as plain regex.
        let json = r#"{"id":"1","name":"A","pattern":"a","dest_template":"{Client}","filename_template":"{Rest}.{Ext}","requires_year":false}"#;
        let r: Rule = serde_json::from_str(json).unwrap();
        assert!(!r.whole_words && r.enabled);
    }

    #[test]
    fn first_match_wins_and_disabled_rules_are_skipped() {
        let mut rules = vec![
            Rule::new("A", "invoice", "{Client}/A", "{Rest}.{Ext}", false),
            Rule::new("B", "invoice|bill", "{Client}/B", "{Rest}.{Ext}", false),
            Rule::catch_all(),
        ];
        let r = CompiledRules::new(&rules).unwrap();
        assert_eq!(
            matched(&r, "INVOICE 7"),
            "A",
            "case-insensitive, first wins"
        );
        assert_eq!(matched(&r, "bill"), "B");
        rules[0].enabled = false;
        let r = CompiledRules::new(&rules).unwrap();
        assert_eq!(matched(&r, "invoice"), "B");
        assert_eq!(matched(&r, "zzz"), CATCH_ALL_NAME);
        assert!(r.first_specific_match("zzz").is_none());
    }

    #[test]
    fn evaluate_fills_templates_including_csv_extras() {
        let rule = Rule::new(
            "Payroll",
            "payroll",
            "{Client}/Payroll/{Partner}/{Year}",
            "{Year} {DocType} {ID} - {Rest}.{Ext}",
            true,
        );
        assert!(validate_rule(&rule, &["Partner".into()]).is_empty());
        let p = parsed("Smith, John_1234_2025 payroll Q4.pdf");
        let hit = evaluate_rule(&rule, &p, &client(), p.year);
        let r = hit.outcome.unwrap();
        assert_eq!(r.dest_rel, Path::new("Payroll").join("KW").join("2025"));
        assert_eq!(r.filename, "2025 Payroll 1234 - 2025 payroll Q4.pdf");
    }

    #[test]
    fn starter_rule_output() {
        let rules = CompiledRules::new(&tax_rules()).unwrap();
        let p = parsed("Smith, John_1234_2025_1040.pdf");
        let r = evaluate(&rules, &p, &client(), p.year).outcome.unwrap();
        assert_eq!(r.dest_rel, Path::new("Tax Returns").join("2025"));
        assert_eq!(r.filename, "2025 Tax Return - Smith, John.pdf");

        let p = parsed("Smith, John_1234_misc scan.pdf");
        let hit = evaluate(&rules, &p, &client(), None);
        assert_eq!(hit.rule_name, CATCH_ALL_NAME);
        assert!(hit.needs_year());
        let r = evaluate(&rules, &p, &client(), Some(2024)).outcome.unwrap();
        assert_eq!(r.dest_rel, Path::new("Source Documents").join("2024"));
        assert_eq!(r.filename, "misc scan.pdf");
    }

    #[test]
    fn requires_year_without_year_template() {
        let rule = Rule::new(
            "Letters",
            "letter",
            "{Client}/Correspondence",
            "{Rest}.{Ext}",
            false,
        );
        let p = parsed("Smith, John_1234_letter.pdf");
        let r = evaluate_rule(&rule, &p, &client(), None).outcome.unwrap();
        assert_eq!(r.dest_rel, Path::new("Correspondence"));
        assert_eq!(r.filename, "letter.pdf");
        let needs = Rule {
            requires_year: true,
            ..rule
        };
        assert!(evaluate_rule(&needs, &p, &client(), None).needs_year());
    }

    #[test]
    fn rule_validation_errors() {
        let base = Rule::new("X", "x", "{Client}/Docs", "{Rest}.{Ext}", false);
        let errs = |r: Rule| validate_rule(&r, &[]);
        assert!(errs(base.clone()).is_empty());
        assert!(
            !errs(Rule {
                pattern: "(".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                pattern: " ".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                dest_template: "Docs/{Client}".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                dest_template: "{Client}/{Nope}".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                dest_template: "{Client}/..".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                filename_template: "{Rest}".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                filename_template: "a/{Rest}.{Ext}".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                dest_template: "{Client}/{Year}".into(),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            !errs(Rule {
                name: " ".into(),
                ..base.clone()
            })
            .is_empty()
        );
        let dup = vec![
            base.clone(),
            Rule {
                id: "2".into(),
                ..base.clone()
            },
            Rule::catch_all(),
        ];
        assert!(
            validate_profile(&dup, &[])
                .iter()
                .any(|(_, e)| e.contains("already named"))
        );
    }

    #[test]
    fn normalize_repairs_catch_all_and_active() {
        let mut f = RulesFile {
            profiles: [(
                "P".to_string(),
                vec![
                    Rule::catch_all(),
                    Rule::new("A", "a", "{Client}", "{Rest}.{Ext}", false),
                ],
            )]
            .into(),
            active: "missing".into(),
        };
        f.profiles.get_mut("P").unwrap()[0].pattern = "changed".into();
        f.normalize();
        assert_eq!(f.active, "P");
        let rules = f.active_rules();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].name, "A");
        assert!(rules[1].catch_all);
        assert_eq!(rules[1].pattern, ".*");

        let mut no_catch = RulesFile {
            profiles: [(
                "Q".to_string(),
                vec![Rule::new("A", "a", "{Client}", "{Rest}.{Ext}", false)],
            )]
            .into(),
            active: "Q".into(),
        };
        no_catch.normalize();
        assert!(no_catch.active_rules().last().unwrap().catch_all);

        let mut empty = RulesFile {
            profiles: BTreeMap::new(),
            active: String::new(),
        };
        empty.normalize();
        assert_eq!(empty, RulesFile::default().with_same_ids(&empty));
    }

    impl RulesFile {
        /// Default file but with `other`'s rule ids, for comparing structure.
        fn with_same_ids(mut self, other: &RulesFile) -> RulesFile {
            for (name, rules) in self.profiles.iter_mut() {
                if let Some(o) = other.profiles.get(name) {
                    for (r, or) in rules.iter_mut().zip(o) {
                        r.id = or.id.clone();
                    }
                }
            }
            self
        }
    }

    #[test]
    fn move_and_add_keep_catch_all_last() {
        let mut rules = tax_rules();
        let names = |r: &[Rule]| r.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        move_rule(&mut rules, 0, 4);
        assert_eq!(
            names(&rules),
            [
                "K-1",
                "Organizer",
                "Engagement Letter",
                "Tax Return",
                "K-1 package",
                CATCH_ALL_NAME
            ]
        );
        move_rule(&mut rules, 4, 99);
        assert!(rules.last().unwrap().catch_all, "can't pass the catch-all");
        move_rule(&mut rules, 5, 0);
        assert!(rules.last().unwrap().catch_all, "catch-all doesn't move");
        let i = add_rule(&mut rules);
        assert_eq!(i, 5);
        assert!(rules.last().unwrap().catch_all);
    }

    #[test]
    fn seed_save_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let (rules, ok, err) = load_or_seed(dir.path());
        assert!(ok && err.is_none());
        assert!(path(dir.path()).exists());
        let (again, _, _) = load_or_seed(dir.path());
        assert_eq!(again, rules, "ids persist");

        std::fs::write(path(dir.path()), "nope").unwrap();
        let (_, ok, err) = load_or_seed(dir.path());
        assert!(!ok && err.is_some());
        assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), "nope");
    }
}
