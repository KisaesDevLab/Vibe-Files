//! Print-capture filename parsers (P02 step 5): editable TOML profiles in
//! `<cabinet>/_Vibe/parsers/*.toml`, shared by every workstation. A profile's `pattern` is a
//! regex over the file name with named groups `id` (client-list ID), `year`, `doc` (the
//! program's document name); `[[doc]]` rules map the document name to a firm doc type.
//!
//! Only UltraTax CS is seeded (QUESTIONS Q-09), for the names this office's UltraTax prints:
//! `<Client name>_<Tax year>_<Form>_<Document>[_<K-1 recipient>]_<Client ID>.pdf` (owner's
//! sample names, 2026-11-03; only their shape is kept here).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const DIR: &str = "parsers";

/// One profile file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Regex over the whole file name, with named groups `id`, `year`, `doc`.
    pub pattern: String,
    /// Source recorded on the document.
    #[serde(default = "firm")]
    pub source: String,
    /// First match wins.
    #[serde(default)]
    pub doc: Vec<DocRule>,
    /// Where a doc type's prints are filed, below the client folder (`Correspondence/{Year}`),
    /// when not the doc type's own default folder (Admin › Doc types). By doc type name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub folders: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRule {
    /// Regex over the `doc` group.
    #[serde(rename = "match")]
    pub pattern: String,
    /// Doc type name (as in Admin › Doc types).
    pub doc_type: String,
}

fn yes() -> bool {
    true
}
fn firm() -> String {
    "firm".into()
}

pub const ULTRATAX: &str = r#"# UltraTax CS: PDFs as this office prints them (owner 2026-11-03)
#   <Client name>_<Tax year>_<Form>_<Document>[_<K-1 recipient>]_<Client ID>.pdf
#   e.g. Harbor Point Farms LLC_2025_1120S_K1_Package_Dana Reyes_HARB1234.pdf
#        Reyes, Dana & Lee_2025_1040_GovernmentCopyTaxReturn_REYE5521.pdf
# `doc` is "<Form>_<Document>" so rules can tell a 1120S K-1 from a 1065 one; a recipient
# is the part before the ID that has a space in it (a person's name).
# Edit freely; first [[doc]] whose `match` finds the document name sets the doc type.
name = "UltraTax CS"
enabled = true
pattern = '^(?P<client>.+?)_(?P<year>(?:19|20)\d{2})_(?P<doc>(?P<form>[^_]+)_.+?)(?:_(?P<recipient>[^_]* [^_]*))?_(?P<id>[^_ ]+)\.pdf$'
source = "firm"

[[doc]]
match = '(?i)^1120-?S_.*K-?1'
doc_type = "K-1 (1120-S)"

[[doc]]
match = '(?i)^1065_.*K-?1'
doc_type = "K-1 (1065)"

[[doc]]
match = '(?i)^1041_.*K-?1'
doc_type = "K-1 (1041)"

[[doc]]
match = '(?i)\b(state|[A-Z]{2} return)\b'
doc_type = "Return copy — state"

[[doc]]
match = '(?i)client.?copy|government.?copy|preparer.?copy|filing.?copy|federal|return'
doc_type = "Return copy — federal"

[[doc]]
match = '(?i)8879'
doc_type = "8879 e-file authorization"

[[doc]]
match = '(?i)organizer'
doc_type = "Organizer"

[[doc]]
match = '(?i)extension|4868|7004'
doc_type = "Extension"

[[doc]]
match = '(?i)estimate|voucher|1040-?ES'
doc_type = "Estimate vouchers"

[[doc]]
match = '(?i)engagement'
doc_type = "Engagement letter"
"#;

/// The profile seeded before the office's real names were known (`<ID>_<Year>_<Doc>.pdf`).
/// A cabinet still holding it unedited gets [`ULTRATAX`] instead.
const ULTRATAX_V1: &str = r#"# UltraTax CS: PDFs printed with its default PDF file name
#   <Client ID>_<Tax year>_<Document name>.pdf     e.g. 1042_2025_Client Copy.pdf
# (UltraTax: Setup > Office Configuration > Print Options > PDF File Options, positions 1-3.)
# Edit freely; first [[doc]] whose `match` finds the document name sets the doc type.
name = "UltraTax CS"
enabled = true
pattern = '^(?P<id>[^_]+)_(?P<year>\d{4})_(?P<doc>.+)\.pdf$'
source = "firm"

[[doc]]
match = '(?i)\b(state|[A-Z]{2} return)\b'
doc_type = "Return copy — state"

[[doc]]
match = '(?i)client copy|government copy|preparer copy|filing copy|federal|return'
doc_type = "Return copy — federal"

[[doc]]
match = '(?i)8879'
doc_type = "8879 e-file authorization"

[[doc]]
match = '(?i)organizer'
doc_type = "Organizer"

[[doc]]
match = '(?i)extension|4868|7004'
doc_type = "Extension"

[[doc]]
match = '(?i)estimate|voucher|1040-?ES'
doc_type = "Estimate vouchers"

[[doc]]
match = '(?i)engagement'
doc_type = "Engagement letter"
"#;

/// Every profile file with its path, for the editor. Files that do not read are reported.
pub fn load_files(root: &Path) -> (Vec<(PathBuf, Profile)>, Vec<String>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(root))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let (mut out, mut errors) = (Vec::new(), Vec::new());
    for f in files {
        match std::fs::read_to_string(&f)
            .map_err(|e| e.to_string())
            .and_then(|t| toml::from_str::<Profile>(&t).map_err(|e| e.to_string()))
        {
            Ok(p) => out.push((f, p)),
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
    (out, errors)
}

/// Writes `profile` to `path` (in the parsers folder) after checking it compiles. The file's
/// comments are replaced by a header: the editor in Admin is the way to change it.
pub fn save_profile(path: &Path, profile: &Profile) -> Result<(), String> {
    let body = toml::to_string_pretty(profile).map_err(|e| e.to_string())?;
    compile(&body)?;
    let text = format!(
        "# Print capture parser profile. Edited in Vibe Files: Admin › Print capture & parsers.\n\
         # `pattern` reads the printed file name (groups id, year, doc, recipient); the first\n\
         # [[doc]] whose `match` finds the document name sets the doc type; [folders] files a doc\n\
         # type's prints somewhere other than its default folder.\n{body}"
    );
    vf_fs::cabinet::write_replace(path, text.as_bytes())
}

/// Filer rules Vibe Files writes for print captures carry this id prefix; they are rewritten
/// from the parser profiles and doc types, never edited by hand.
pub const MANAGED_PREFIX: &str = "vf-capture-";

/// Where each doc type of `profile` is filed: its override in `[folders]`, else the doc type's
/// default folder. Doc types with neither are left to Filer's own rules.
pub fn folder_for(profile: &Profile, doc_type: &str, types: &[vf_fs::doctypes::DocType]) -> String {
    profile
        .folders
        .get(doc_type)
        .filter(|f| !f.trim().is_empty())
        .cloned()
        .or_else(|| {
            types
                .iter()
                .find(|t| t.name == doc_type)
                .map(|t| t.default_folder.clone())
        })
        .unwrap_or_default()
        .trim()
        .trim_matches('/')
        .to_string()
}

/// The Filer rules that file each mapped doc type of the enabled profiles in its folder.
/// A capture's delivered name reads `<year> <profile> <doc type> …` (see the pipeline), so each
/// rule matches its profile and doc type there. File name and next-year folder follow Filer's
/// `UltraTax` rule when the profile has one.
pub fn filer_rules(
    profiles: &[Profile],
    types: &[vf_fs::doctypes::DocType],
    like: Option<&filer_core::rules::Rule>,
) -> Vec<filer_core::rules::Rule> {
    use filer_core::rules::Rule;
    let mut out: Vec<Rule> = Vec::new();
    for p in profiles.iter().filter(|p| p.enabled) {
        let mut seen = std::collections::HashSet::new();
        for d in &p.doc {
            if !seen.insert(d.doc_type.clone()) {
                continue;
            }
            let folder = folder_for(p, &d.doc_type, types);
            if folder.is_empty() {
                continue;
            }
            let by_year = folder.contains("{Year}");
            let words = crate::pipeline::clean(&format!("{} {}", p.name, d.doc_type));
            let pattern = words
                .split_whitespace()
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(r"\s+");
            // Rule names are unique in a profile (Filer has its own "Engagement Letter").
            let mut r = Rule::new(
                &format!("{} · {}", d.doc_type, p.name),
                &pattern,
                &format!("{{Client}}/{folder}"),
                like.map_or("{Original}.{Ext}", |l| l.filename_template.as_str()),
                by_year,
            );
            r.id = format!("{MANAGED_PREFIX}{}", stable_id(&p.name, &d.doc_type));
            r.next_year_folder = by_year && like.is_some_and(|l| l.next_year_folder);
            out.push(r);
        }
    }
    out
}

/// A stable id from the profile and doc type (so an unchanged mapping rewrites nothing).
fn stable_id(profile: &str, doc_type: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("{profile}\u{0}{doc_type}").as_bytes());
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Brings Filer's `rules.json` in line with the parser profiles and doc types: the managed rules
/// are removed from every profile and written again into the active one, just before its
/// `UltraTax` rule (else before the catch-all). Filer's own rules are never touched. Returns
/// whether the file changed (Intake must then reload its rules).
pub fn sync_filer_rules(root: &Path) -> Result<bool, String> {
    let master = vf_fs::clients::master_dir(root);
    let (mut file, savable, err) = filer_core::rules::load_or_seed(&master);
    if !savable {
        return Err(err.unwrap_or_else(|| "rules.json cannot be read".into()));
    }
    let before = file.clone();
    let (profiles, _) = load_files(root);
    let profiles: Vec<Profile> = profiles.into_iter().map(|(_, p)| p).collect();
    let types = vf_fs::doctypes::load(root)?.unwrap_or_default();
    for rules in file.profiles.values_mut() {
        rules.retain(|r| !r.id.starts_with(MANAGED_PREFIX));
    }
    let active = file.active_rules_mut();
    let ut = active
        .iter()
        .position(|r| r.name.eq_ignore_ascii_case("UltraTax"));
    let made = filer_rules(&profiles, &types, ut.map(|i| &active[i]));
    let at = ut.unwrap_or_else(|| {
        active
            .iter()
            .position(|r| r.catch_all)
            .unwrap_or(active.len())
    });
    for (k, r) in made.into_iter().enumerate() {
        active.insert(at + k, r);
    }
    if file == before {
        return Ok(false);
    }
    let errors = filer_core::rules::validate_file(&file, &[]);
    if !errors.is_empty() {
        return Err(format!(
            "the folders give Filer rules it refuses: {}",
            errors.join("; ")
        ));
    }
    filer_core::rules::save(&master, &file).map_err(|e| format!("saving rules.json: {e}"))?;
    Ok(true)
}

/// Where a printed file would go, for the editor's test box: what the profile reads, and the
/// folder and name Filer's rules give it (or why not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trial {
    pub parsed: Option<Parsed>,
    /// The name it reaches Filer's inbox under.
    pub delivered: String,
    /// Filer's rule, and the folder below the client folder plus the file name.
    pub rule: String,
    pub destination: Result<String, String>,
}

/// [`Trial`] for `file_name` with the profiles as saved and Filer's rules as saved.
pub fn trial(root: &Path, file_name: &str) -> Trial {
    let lookup = crate::pipeline::Lookup::load(root).unwrap_or(crate::pipeline::Lookup {
        clients: Default::default(),
        doc_types: Default::default(),
    });
    let (profiles, _) = load(root);
    let parsed = parse(&profiles, file_name);
    let (delivered, _, _) = crate::pipeline::capture_name(root, &lookup, file_name);
    let master = vf_fs::clients::master_dir(root);
    let (rules, _, _) = filer_core::rules::load_or_seed(&master);
    let settings = filer_intake::config::Settings::load_or_default(&master).0;
    let id_pattern = if settings.id_pattern.is_empty() {
        filer_core::parse::DEFAULT_ID_PATTERN.to_string()
    } else {
        settings.id_pattern.clone()
    };
    let year: i32 = vf_core::now_rfc3339()
        .get(..4)
        .and_then(|y| y.parse().ok())
        .unwrap_or(2026);
    let fail = |rule: String, e: String| Trial {
        parsed: parsed.clone(),
        delivered: delivered.clone(),
        rule,
        destination: Err(e),
    };
    let re = match filer_core::parse::filename_regex(&id_pattern) {
        Ok(r) => r,
        Err(e) => return fail(String::new(), format!("Filer's ID pattern: {e}")),
    };
    let fp = filer_core::parse::parse_filename(&delivered, &re, year);
    let compiled = match filer_core::rules::CompiledRules::new(rules.active_rules()) {
        Ok(c) => c,
        Err(e) => return fail(String::new(), format!("Filer's rules: {e}")),
    };
    let rule = compiled.first_match(&fp.rest).name.clone();
    let Some(id) = fp.id.clone() else {
        return fail(
            rule,
            "no client ID in the name: Intake asks which client".into(),
        );
    };
    let client = filer_core::clients::Client {
        uuid: uuid::Uuid::nil(),
        id: id.clone(),
        name: lookup
            .clients
            .get(&id)
            .cloned()
            .unwrap_or_else(|| fp.name.clone()),
        extra: Default::default(),
    };
    if !lookup.clients.contains_key(&id) {
        return fail(rule, format!("client ID {id} is not in the client list"));
    }
    let hit = filer_core::rules::evaluate(&compiled, &fp, &client, fp.year);
    Trial {
        parsed: parsed.clone(),
        delivered: delivered.clone(),
        rule: hit.rule_name,
        destination: match hit.outcome {
            Ok(r) => {
                let mut d = r.dest_rel.to_string_lossy().replace('\\', "/");
                if !d.is_empty() {
                    d.push('/');
                }
                Ok(format!("{d}{}", r.filename))
            }
            Err(filer_core::rules::HitProblem::NeedsYear) => {
                Err("no tax year in the name: Intake asks for one".into())
            }
            Err(e) => Err(format!("{e:?}")),
        },
    }
}

pub fn dir(root: &Path) -> PathBuf {
    vf_fs::clients::master_dir(root).join(DIR)
}

/// Writes the seeded profile when the folder has none, and replaces an unedited copy of the
/// first seeded UltraTax profile. Returns whether it wrote.
pub fn seed_if_missing(root: &Path) -> Result<bool, String> {
    let d = dir(root);
    let ut = d.join("ultratax.toml");
    if std::fs::read_to_string(&ut).is_ok_and(|t| t.replace("\r\n", "\n") == ULTRATAX_V1) {
        vf_fs::cabinet::write_replace(&ut, ULTRATAX.as_bytes())?;
        return Ok(true);
    }
    let any = std::fs::read_dir(&d)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .any(|e| e.path().extension().is_some_and(|x| x == "toml"))
        })
        .unwrap_or(false);
    if any {
        return Ok(false);
    }
    std::fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    vf_fs::cabinet::write_new(&d.join("ultratax.toml"), ULTRATAX.as_bytes())?;
    Ok(true)
}

/// A profile ready to use.
pub struct Compiled {
    pub profile: Profile,
    re: Regex,
    doc: Vec<(Regex, String)>,
}

/// Every profile file, by file name. A file that does not read or compile is reported, not
/// fatal.
pub fn load(root: &Path) -> (Vec<Compiled>, Vec<String>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(root))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let (mut out, mut errors) = (Vec::new(), Vec::new());
    for f in files {
        match std::fs::read_to_string(&f)
            .map_err(|e| e.to_string())
            .and_then(|t| compile(&t))
        {
            Ok(c) => out.push(c),
            Err(e) => errors.push(format!("{}: {e}", f.display())),
        }
    }
    (out, errors)
}

pub fn compile(text: &str) -> Result<Compiled, String> {
    let profile: Profile = toml::from_str(text).map_err(|e| e.to_string())?;
    let re = Regex::new(&profile.pattern).map_err(|e| format!("pattern: {e}"))?;
    if !re.capture_names().flatten().any(|n| n == "id") {
        return Err("pattern needs a (?P<id>…) group".into());
    }
    let doc = profile
        .doc
        .iter()
        .map(|d| {
            Regex::new(&d.pattern)
                .map(|r| (r, d.doc_type.clone()))
                .map_err(|e| format!("match {:?}: {e}", d.pattern))
        })
        .collect::<Result<_, _>>()?;
    Ok(Compiled { profile, re, doc })
}

/// What a profile read out of a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub profile: String,
    pub client_id: String,
    pub year: Option<i32>,
    /// The program's document name (`1120S_K1_Package`).
    pub doc: Option<String>,
    /// Who a K-1 is for, when the name carries it.
    pub recipient: Option<String>,
    /// Doc type name from the first matching `[[doc]]` rule.
    pub doc_type: Option<String>,
    pub source: String,
}

impl Compiled {
    pub fn parse(&self, file_name: &str) -> Option<Parsed> {
        if !self.profile.enabled {
            return None;
        }
        let c = self.re.captures(file_name)?;
        let doc = c.name("doc").map(|m| m.as_str().trim().to_string());
        Some(Parsed {
            profile: self.profile.name.clone(),
            client_id: c.name("id")?.as_str().trim().to_string(),
            year: c.name("year").and_then(|m| m.as_str().parse().ok()),
            doc_type: doc.as_ref().and_then(|d| {
                self.doc
                    .iter()
                    .find(|(r, _)| r.is_match(d))
                    .map(|(_, t)| t.clone())
            }),
            doc,
            recipient: c
                .name("recipient")
                .map(|m| m.as_str().trim().to_string())
                .filter(|r| !r.is_empty()),
            source: self.profile.source.clone(),
        })
    }
}

/// The first enabled profile that reads `file_name`.
pub fn parse(profiles: &[Compiled], file_name: &str) -> Option<Parsed> {
    profiles.iter().find_map(|p| p.parse(file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes of the office's UltraTax names (owner, 2026-11-03), with invented names.
    #[test]
    fn ultratax_office_names() {
        let p = compile(ULTRATAX).unwrap();
        let got = p
            .parse("Harbor Point Farms LLC_2025_1120S_K1_Package_Dana Reyes_HARB1234.pdf")
            .unwrap();
        assert_eq!(got.client_id, "HARB1234");
        assert_eq!(got.year, Some(2025));
        assert_eq!(got.doc.as_deref(), Some("1120S_K1_Package"));
        assert_eq!(got.recipient.as_deref(), Some("Dana Reyes"));
        assert_eq!(got.doc_type.as_deref(), Some("K-1 (1120-S)"));
        assert_eq!(got.source, "firm");
        let one = |n: &str| p.parse(n).unwrap();
        let t = |n: &str| one(n).doc_type;
        for (name, doc, recipient, doc_type) in [
            (
                "Harbor Point Farms LLC_2025_1120S_GovernmentCopyK1Package_Dana Reyes_HARB1234.pdf",
                "1120S_GovernmentCopyK1Package",
                Some("Dana Reyes"),
                "K-1 (1120-S)",
            ),
            (
                "Harbor Point Farms LLC_2025_1120S_Preparer_Copy_K1_Dana Reyes_HARB1234.pdf",
                "1120S_Preparer_Copy_K1",
                Some("Dana Reyes"),
                "K-1 (1120-S)",
            ),
            (
                "Harbor Point Farms LLC_2025_1120S_GovernmentCopyTaxReturn_HARB1234.pdf",
                "1120S_GovernmentCopyTaxReturn",
                None,
                "Return copy — federal",
            ),
            (
                "Harbor Point Farms LLC_2025_1120S_Preparer_Copy_Tax_Return_HARB1234.pdf",
                "1120S_Preparer_Copy_Tax_Return",
                None,
                "Return copy — federal",
            ),
            (
                "Harbor Point Farms LLC_2025_1120S_Tax_Return_HARB1234.pdf",
                "1120S_Tax_Return",
                None,
                "Return copy — federal",
            ),
            (
                "Reyes, Dana & Lee_2025_1040_GovernmentCopyTaxReturn_REYE5521.pdf",
                "1040_GovernmentCopyTaxReturn",
                None,
                "Return copy — federal",
            ),
            (
                "Reyes, Dana & Lee_2025_1040_Tax_Return_REYE5521.pdf",
                "1040_Tax_Return",
                None,
                "Return copy — federal",
            ),
            (
                "Reyes, Dana & Lee_2025_1040_Preparer_Copy_Tax_Return_REYE5521.pdf",
                "1040_Preparer_Copy_Tax_Return",
                None,
                "Return copy — federal",
            ),
        ] {
            let g = one(name);
            assert_eq!(g.doc.as_deref(), Some(doc), "{name}");
            assert_eq!(g.recipient.as_deref(), recipient, "{name}");
            assert_eq!(g.doc_type.as_deref(), Some(doc_type), "{name}");
            assert!(
                g.client_id == "HARB1234" || g.client_id == "REYE5521",
                "{name}"
            );
            assert_eq!(g.year, Some(2025), "{name}");
        }
        assert_eq!(
            t("Cedar Ridge Partners_2025_1065_K1_Package_Sam Ortiz_CEDA7788.pdf").as_deref(),
            Some("K-1 (1065)")
        );
        assert_eq!(
            t("Reyes, Dana & Lee_2025_1040_Form 8879_REYE5521.pdf").as_deref(),
            Some("8879 e-file authorization")
        );
        assert_eq!(
            t("Reyes, Dana & Lee_2025_1040_Invoice_REYE5521.pdf"),
            None,
            "no rule: no doc type"
        );
        assert!(p.parse("scan0001.pdf").is_none());
        assert!(p.parse("Reyes_25_1040_Tax_Return_REYE5521.pdf").is_none());
    }

    /// A cabinet seeded with the first profile and never edited gets the new one; an edited
    /// one is left alone.
    #[test]
    fn unedited_first_profile_is_upgraded() {
        let t = tempfile::tempdir().unwrap();
        let d = dir(t.path());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("ultratax.toml"), ULTRATAX_V1.replace('\n', "\r\n")).unwrap();
        assert!(seed_if_missing(t.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(d.join("ultratax.toml")).unwrap(),
            ULTRATAX
        );
        let edited = ULTRATAX_V1.replace("enabled = true", "enabled = false");
        std::fs::write(d.join("ultratax.toml"), &edited).unwrap();
        assert!(!seed_if_missing(t.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(d.join("ultratax.toml")).unwrap(),
            edited
        );
    }

    #[test]
    fn seeds_once_and_reports_bad_files() {
        let t = tempfile::tempdir().unwrap();
        assert!(seed_if_missing(t.path()).unwrap());
        assert!(!seed_if_missing(t.path()).unwrap());
        std::fs::write(
            dir(t.path()).join("bad.toml"),
            "name = \"x\"\npattern = '('",
        )
        .unwrap();
        let (ok, errors) = load(t.path());
        assert_eq!(ok.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(
            compile("name='x'\npattern='^a$'").is_err(),
            "needs an id group"
        );
    }
}
