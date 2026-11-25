//! Where letter templates are kept: `<cabinet>/_Vibe/letters/<id>.json` (the current version)
//! and `_Vibe/letters/history/<id>.v<N>.json` (each version saved), so a delivery that records
//! the version it used can be read back. Seeded once with samples marked "Sample — review
//! before use" (§12.2): engagement letters with headings for the firm's own text, §7216
//! consents, and the Advantage and Concierge package addenda.

use std::path::{Path, PathBuf};

use crate::{Block, Template};

pub const DIR: &str = "letters";
const SEEDED: &str = ".seeded";

pub fn dir(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(DIR)
}

/// A template id as a file name allows (letters, digits, `-`).
pub fn safe_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// Every template, by name.
pub fn load_all(root: &Path) -> Result<Vec<Template>, String> {
    let rd = match std::fs::read_dir(dir(root)) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("the letter templates cannot be read: {e}")),
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        match serde_json::from_str::<Template>(&text) {
            Ok(t) => out.push(t),
            Err(e) => return Err(format!("{} cannot be read: {e}", p.display())),
        }
    }
    out.sort_by_key(|t| t.name.to_lowercase());
    Ok(out)
}

/// Saves `t` as its next version (and keeps that version in the history). Answers it saved.
pub fn save(root: &Path, t: &Template) -> Result<Template, String> {
    if t.name.trim().is_empty() {
        return Err("Give the letter a name.".into());
    }
    let id = safe_id(&t.id);
    if id.is_empty() {
        return Err("The letter has no id.".into());
    }
    vf_fs::lock::with(root, "letters", "the letter templates", || {
        let path = dir(root).join(format!("{id}.json"));
        let current: Option<Template> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let saved = Template {
            id: id.clone(),
            version: current.map_or(1, |c| c.version + 1).max(t.version + 1),
            ..t.clone()
        };
        let bytes = serde_json::to_vec_pretty(&saved).expect("serializes");
        let hist = dir(root).join("history");
        std::fs::create_dir_all(&hist).map_err(|e| e.to_string())?;
        vf_fs::cabinet::write_replace(&hist.join(format!("{id}.v{}.json", saved.version)), &bytes)?;
        vf_fs::cabinet::write_replace(&path, &bytes)?;
        Ok(saved)
    })
}

/// A saved version of a template (what a delivery used).
pub fn version(root: &Path, id: &str, version: u32) -> Option<Template> {
    let p = dir(root)
        .join("history")
        .join(format!("{}.v{version}.json", safe_id(id)));
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Removes a template (its history stays).
pub fn delete(root: &Path, id: &str) -> Result<(), String> {
    let p = dir(root).join(format!("{}.json", safe_id(id)));
    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Writes the samples once (a firm that deletes them does not get them back). Whether it did.
pub fn seed_if_missing(root: &Path) -> Result<bool, String> {
    let marker = dir(root).join(SEEDED);
    if marker.exists() || !load_all(root)?.is_empty() {
        refresh_samples(root)?;
        return Ok(false);
    }
    std::fs::create_dir_all(dir(root)).map_err(|e| e.to_string())?;
    for t in samples() {
        save(root, &t)?;
    }
    std::fs::write(&marker, b"").map_err(|e| e.to_string())?;
    Ok(true)
}

/// Fields no longer offered (owner 2026-11-24, Q25).
const DROPPED: [&str; 2] = ["{Preparer}", "{Client address}"];

/// Seeded letters still marked sample (the firm never saved its own text) that use a field no
/// longer offered are replaced by today's sample, as a new version. A letter the firm saved is
/// never touched. Returns how many were replaced.
pub fn refresh_samples(root: &Path) -> Result<usize, String> {
    let fresh = samples();
    let mut n = 0;
    for t in load_all(root)?.into_iter().filter(|t| t.sample) {
        let uses =
            serde_json::to_string(&t.blocks).is_ok_and(|s| DROPPED.iter().any(|f| s.contains(f)));
        if let Some(s) = fresh.iter().find(|s| s.id == t.id).filter(|_| uses) {
            save(
                root,
                &Template {
                    version: t.version,
                    ..s.clone()
                },
            )?;
            n += 1;
        }
    }
    Ok(n)
}

fn p(text: &str) -> Block {
    Block::Paragraph {
        text: text.into(),
        only_for: vec![],
    }
}
fn h(text: &str) -> Block {
    Block::Heading {
        text: text.into(),
        only_for: vec![],
    }
}
fn sig(role: &str, only_for: &[&str]) -> Block {
    Block::Signature {
        role: role.into(),
        only_for: only_for.iter().map(|s| s.to_string()).collect(),
    }
}
const ENTITIES: [&str; 3] = ["1120", "1120-S", "1065"];

/// Signature blocks for any return type: taxpayer and spouse on a 1040, an officer or partner
/// with their title on an entity's.
fn signatures() -> Vec<Block> {
    vec![
        sig("taxpayer", &["1040"]),
        sig("spouse", &["1040"]),
        sig("officer", &ENTITIES),
    ]
}

/// The seeded samples (all editable; marked sample until the firm saves its own text).
pub fn samples() -> Vec<Template> {
    let base = |id: &str, name: &str, types: &[&str], doc_type: &str, ticked: bool| Template {
        id: id.into(),
        version: 0,
        name: name.into(),
        return_types: types.iter().map(|s| s.to_string()).collect(),
        kind: "letter".into(),
        package: None,
        doc_type: doc_type.into(),
        letterhead: true,
        default_ticked: ticked,
        sample: true,
        blocks: vec![],
    };
    let mut out = Vec::new();
    let mut e = base(
        "engagement-1040",
        "Engagement letter",
        &["1040"],
        "Engagement letter",
        true,
    );
    e.blocks = vec![
        p("{Date}"),
        p("{Client name}"),
        h("Engagement letter for your {Tax year} {Return type}"),
        p("Dear {Signer first names},"),
        p(
            "Paste your firm's engagement letter text here: the services, what you will provide, what we will provide, and how long this engagement lasts.",
        ),
        h("Fees"),
        p("Our fee for preparing your {Tax year} returns is {Fee}."),
        h("Agreement"),
        p("By signing below you agree to the terms of this letter."),
        p("{Firm name}"),
        sig("taxpayer", &[]),
        sig("spouse", &[]),
    ];
    out.push(e);
    let mut e = base(
        "engagement-entity",
        "Engagement letter",
        &ENTITIES,
        "Engagement letter",
        true,
    );
    e.blocks = vec![
        p("{Date}"),
        p("{Client name}"),
        h("Engagement letter for the {Tax year} Form {Return type}"),
        p("Dear {Signer first names},"),
        p(
            "Paste your firm's engagement letter text here: the services, the entity's responsibilities, and how long this engagement lasts.",
        ),
        h("Fees"),
        p("Our fee for preparing the {Tax year} return is {Fee}."),
        h("Agreement"),
        p("By signing below you confirm you may agree to this letter for {Client name}."),
        p("{Firm name}"),
        sig("officer", &[]),
    ];
    out.push(e);
    for (id, name, what) in [
        (
            "consent-disclose",
            "Consent to disclose tax return information",
            "disclose your tax return information to",
        ),
        (
            "consent-use",
            "Consent to use tax return information",
            "use your tax return information for",
        ),
    ] {
        let mut c = base(id, name, &[], "Consent", false);
        c.blocks = vec![
            h(name),
            p(&format!(
                "Federal law requires this consent form be provided to you. Paste the firm's consent wording here: whom or what the firm may {what}, and for how long. The required wording is the firm's to confirm against Treas. Reg. §301.7216-3 and Rev. Proc. 2013-14."
            )),
            p(
                "If you agree, sign below. This consent is valid until {Next year}-12-31 unless a different date is written above.",
            ),
        ];
        c.blocks.extend(signatures());
        out.push(c);
    }
    for (id, pkg) in [
        ("addendum-advantage", "Advantage"),
        ("addendum-concierge", "Concierge"),
    ] {
        let mut a = base(
            id,
            &format!("{pkg} Package Addendum"),
            &[],
            "Engagement letter addendum",
            false,
        );
        a.kind = "addendum".into();
        a.package = Some(pkg.into());
        a.blocks = vec![
            h(&format!("{pkg} Package Addendum to the engagement letter")),
            p(
                "This addendum adds the {Package name} package to your {Tax year} engagement, for a fee of {Package fee}.",
            ),
            p(
                "Paste the reviewed addendum text here (the services the package includes, what it does not include, and how long it lasts).",
            ),
            p("The person choosing signs for both spouses on a joint return."),
        ];
        a.blocks.extend(signatures());
        out.push(a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_with_a_dropped_field_are_refreshed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        assert!(seed_if_missing(root).unwrap());
        // A sample from before Q25, and one the firm saved with the same field.
        let mut old = samples().into_iter().find(|t| t.sample).unwrap();
        old.blocks.push(p("{Firm name}, {Preparer}"));
        save(root, &old).unwrap();
        let mut own = samples().into_iter().rfind(|t| t.sample).unwrap();
        assert_ne!(own.id, old.id);
        own.sample = false;
        own.blocks.push(p("{Preparer}"));
        save(root, &own).unwrap();
        assert_eq!(refresh_samples(root).unwrap(), 1);
        let all = load_all(root).unwrap();
        let s = |id: &str| {
            serde_json::to_string(&all.iter().find(|t| t.id == id).unwrap().blocks).unwrap()
        };
        assert!(!s(&old.id).contains("{Preparer}"));
        assert!(s(&own.id).contains("{Preparer}"));
        assert_eq!(refresh_samples(root).unwrap(), 0);
    }

    #[test]
    fn seeds_once_saves_versions_and_keeps_history() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        assert!(seed_if_missing(root).unwrap());
        let all = load_all(root).unwrap();
        assert_eq!(all.len(), 6);
        assert!(all.iter().all(|t| t.sample && t.version == 1));
        let mut e = all
            .iter()
            .find(|t| t.id == "engagement-1040")
            .unwrap()
            .clone();
        e.sample = false;
        e.name = "Our engagement letter".into();
        let saved = save(root, &e).unwrap();
        assert_eq!(saved.version, 2);
        assert_eq!(
            version(root, "engagement-1040", 1).unwrap().name,
            "Engagement letter"
        );
        assert_eq!(
            version(root, "engagement-1040", 2).unwrap().name,
            "Our engagement letter"
        );
        // Deleted samples are not seeded again.
        for t in load_all(root).unwrap() {
            delete(root, &t.id).unwrap();
        }
        assert!(!seed_if_missing(root).unwrap());
        assert!(load_all(root).unwrap().is_empty());
    }

    #[test]
    fn every_sample_lays_out_for_every_return_type() {
        let mut f = crate::Fields::new();
        f.insert("Taxpayer name".into(), "Jordan Sample".into());
        f.insert("Spouse name".into(), "Casey Sample".into());
        f.insert("Signer name".into(), "Jordan Sample".into());
        for t in samples() {
            for rt in ["1040", "1120", "1120-S", "1065"] {
                if !t.offered_for(rt) {
                    continue;
                }
                let laid = crate::layout(&t, &f, rt, t.package.as_deref());
                assert!(
                    !laid.blocks.is_empty(),
                    "{} for {rt} has no signature",
                    t.id
                );
            }
        }
    }
}
