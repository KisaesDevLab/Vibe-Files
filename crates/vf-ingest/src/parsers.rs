//! Print-capture filename parsers (P02 step 5): editable TOML profiles in
//! `<cabinet>/_Vibe/parsers/*.toml`, shared by every workstation. A profile's `pattern` is a
//! regex over the file name with named groups `id` (client-list ID), `year`, `doc` (the
//! program's document name); `[[doc]]` rules map the document name to a firm doc type.
//!
//! Only UltraTax CS is seeded (QUESTIONS Q-09), for the names this office's UltraTax prints:
//! `<Client name>_<Tax year>_<Form>_<Document>[_<K-1 recipient>]_<Client ID>.pdf` (owner's
//! sample names, 2026-11-03; only their shape is kept here).

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
