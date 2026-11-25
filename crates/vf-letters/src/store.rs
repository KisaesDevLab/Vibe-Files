//! Where letter templates are kept: `<cabinet>/_Vibe/letters/<id>.json` (the current version)
//! and `_Vibe/letters/history/<id>.v<N>.json` (each version saved), so a delivery that records
//! the version it used can be read back. Seeded once with samples marked "Sample — review
//! before use" (§12.2): engagement letters with headings for the firm's own text, §7216
//! consents, and the Advantage and Concierge package addenda. Annual agreements
//! (ADDENDUM-annual-agreements.md, owner 2026-11-25) add the AICPA Annual Tax Compliance Kit's
//! letters, its Terms and Conditions and closure letter (`assets/aicpa`, made by
//! `tools/aicpa/convert.py`), and bookkeeping, payroll and advisory letters (`assets/services`).
//! `.seeded` lists the ids seeded, so a cabinet seeded before gets each new sample once.

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

/// The samples of the first release (an empty `.seeded` from then means these).
const FIRST_SAMPLES: [&str; 6] = [
    "engagement-1040",
    "engagement-entity",
    "consent-disclose",
    "consent-use",
    "addendum-advantage",
    "addendum-concierge",
];

/// Writes each sample once (a firm that deletes one does not get it back). Whether it wrote any.
pub fn seed_if_missing(root: &Path) -> Result<bool, String> {
    let marker = dir(root).join(SEEDED);
    let existing = load_all(root)?;
    let mut seeded: Vec<String> = match std::fs::read_to_string(&marker) {
        Ok(s) if s.trim().is_empty() => FIRST_SAMPLES.iter().map(|s| s.to_string()).collect(),
        Ok(s) => s.lines().map(|l| l.trim().to_string()).collect(),
        // Templates but no marker (from before the marker): the first samples were seeded.
        Err(_) if !existing.is_empty() => FIRST_SAMPLES.iter().map(|s| s.to_string()).collect(),
        Err(_) => Vec::new(),
    };
    if !existing.is_empty() {
        refresh_samples(root)?;
    }
    std::fs::create_dir_all(dir(root)).map_err(|e| e.to_string())?;
    let mut any = false;
    for t in samples() {
        if seeded.contains(&t.id) {
            continue;
        }
        if !existing.iter().any(|x| x.id == t.id) {
            save(root, &t)?;
            any = true;
        }
        seeded.push(t.id.clone());
    }
    if any || !marker.exists() {
        let mut text = seeded.join("\n");
        text.push('\n');
        vf_fs::cabinet::write_replace(&marker, text.as_bytes())?;
    }
    Ok(any)
}

/// The firm's Terms and Conditions (the first template of that kind), appended where a letter
/// says `{Terms and Conditions}`.
pub fn terms(all: &[Template]) -> Option<&Template> {
    all.iter().find(|t| t.kind == crate::TERMS)
}

/// The letter as sent: its `{Terms and Conditions}` replaced by the firm's Terms and Conditions.
pub fn as_sent(t: &Template, all: &[Template]) -> Template {
    crate::with_terms(t, terms(all))
}

/// The built-in letters kept as files: the AICPA kit's (converted) and the service letters.
const ASSETS: [&str; 20] = [
    include_str!("../assets/aicpa/aicpa-1040.json"),
    include_str!("../assets/aicpa/aicpa-1120.json"),
    include_str!("../assets/aicpa/aicpa-1120s.json"),
    include_str!("../assets/aicpa/aicpa-1065.json"),
    include_str!("../assets/aicpa/aicpa-1041.json"),
    include_str!("../assets/aicpa/aicpa-990.json"),
    include_str!("../assets/aicpa/aicpa-990pf.json"),
    include_str!("../assets/aicpa/aicpa-709.json"),
    include_str!("../assets/aicpa/aicpa-706.json"),
    include_str!("../assets/aicpa/aicpa-5500.json"),
    include_str!("../assets/aicpa/aicpa-fbar.json"),
    include_str!("../assets/aicpa/aicpa-amended.json"),
    include_str!("../assets/aicpa/aicpa-projections.json"),
    include_str!("../assets/aicpa/aicpa-consulting.json"),
    include_str!("../assets/aicpa/aicpa-audit-rep.json"),
    include_str!("../assets/aicpa/aicpa-terms.json"),
    include_str!("../assets/aicpa/aicpa-closure.json"),
    include_str!("../assets/services/service-bookkeeping.json"),
    include_str!("../assets/services/service-payroll.json"),
    include_str!("../assets/services/service-advisory.json"),
];

/// Fields no longer offered (owner 2026-11-24, Q25).
const DROPPED: [&str; 2] = ["{Preparer}", "{Client address}"];

/// Seeded letters still marked sample (the firm never saved its own text) whose text differs
/// from today's sample (a field no longer offered, Q25; signers for agreements, 2026-11-25) are
/// replaced by today's sample, as a new version. A letter the firm saved is never touched.
/// Returns how many were replaced.
pub fn refresh_samples(root: &Path) -> Result<usize, String> {
    let fresh = samples();
    let mut n = 0;
    for t in load_all(root)?.into_iter().filter(|t| t.sample) {
        let uses =
            serde_json::to_string(&t.blocks).is_ok_and(|s| DROPPED.iter().any(|f| s.contains(f)));
        if let Some(s) = fresh
            .iter()
            .find(|s| s.id == t.id)
            .filter(|s| uses || s.blocks != t.blocks || s.default_ticked != t.default_ticked)
        {
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

/// Signature blocks for any return type or agreement: taxpayer and spouse for an individual
/// (`Individual`, from `vf_portal::delivery::letter_context`), an officer or partner with their
/// title for an entity.
fn signatures() -> Vec<Block> {
    vec![
        sig("taxpayer", &["1040", "Individual"]),
        sig("spouse", &["1040", "Individual"]),
        sig("officer", &["1120", "1120-S", "1065", "Entity"]),
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
    for s in ASSETS {
        out.push(serde_json::from_str(s).expect("a built-in letter reads"));
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
        assert_eq!(all.len(), samples().len());
        assert_eq!(all.len(), 26);
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
    fn a_cabinet_seeded_before_gets_the_new_samples_once() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(dir(root)).unwrap();
        // Seeded by the first release: its six samples and an empty marker; one was deleted.
        for s in samples()
            .into_iter()
            .filter(|s| FIRST_SAMPLES.contains(&s.id.as_str()) && s.id != "consent-use")
        {
            save(root, &s).unwrap();
        }
        std::fs::write(dir(root).join(SEEDED), b"").unwrap();
        assert!(seed_if_missing(root).unwrap());
        let all = load_all(root).unwrap();
        assert_eq!(all.len(), 25, "the new 20, not the deleted consent");
        assert!(all.iter().any(|t| t.id == "aicpa-terms"));
        assert!(!all.iter().any(|t| t.id == "consent-use"));
        // A new sample the firm deleted is not seeded again either.
        delete(root, "service-payroll").unwrap();
        assert!(!seed_if_missing(root).unwrap());
        assert_eq!(load_all(root).unwrap().len(), 24);
    }

    /// The kit's letters as converted: each named fill-in left for the firm, nothing of the
    /// kit's cover, notes or disclaimer (A16), and the Terms and Conditions where they go (A5).
    #[test]
    fn the_kit_letters_match_the_converters_manifest() {
        #[derive(serde::Deserialize)]
        struct M {
            id: String,
            blocks: usize,
            fill_ins: Vec<String>,
        }
        let manifest: Vec<M> =
            serde_json::from_str(include_str!("../../../tools/aicpa/manifest.json")).unwrap();
        assert_eq!(manifest.len(), 17);
        let all = samples();
        let terms = all.iter().find(|t| t.kind == crate::TERMS).unwrap();
        for m in &manifest {
            let t = all.iter().find(|t| t.id == m.id).expect("in the samples");
            assert_eq!(t.blocks.len(), m.blocks, "{}", m.id);
            let mut f: Vec<String> = Vec::new();
            for b in &t.blocks {
                for s in b.texts() {
                    f.extend(crate::fill_ins_in(s));
                }
            }
            f.sort();
            f.dedup();
            assert_eq!(f, m.fill_ins, "{}", m.id);
            let text = serde_json::to_string(&t.blocks).unwrap();
            for gone in [
                "CNA",
                "How to use this document",
                "INITIAL HERE",
                "On the rare occasion",
            ] {
                assert!(!text.contains(gone), "{} still has {gone}", m.id);
            }
            if t.kind == crate::LETTER {
                let sent = as_sent(t, &all);
                assert!(sent.blocks.len() > t.blocks.len(), "{}: no Terms", m.id);
                assert!(
                    !serde_json::to_string(&sent.blocks)
                        .unwrap()
                        .contains("{Terms and")
                );
                assert!(
                    t.blocks
                        .iter()
                        .any(|b| matches!(b, crate::Block::Signature { .. })),
                    "{}: no signature",
                    m.id
                );
            }
        }
        assert!(terms.blocks.len() > 50);
    }

    /// The longest kit letter with the Terms and Conditions (about 11,000 words) lays out in
    /// well under the portal's 15 MB letter limit, quickly, with its signatures on the last page.
    #[test]
    fn the_longest_letter_with_terms_lays_out() {
        let all = samples();
        let t = all.iter().find(|t| t.id == "aicpa-1065").unwrap();
        let mut f = crate::Fields::new();
        f.insert("Signer name".into(), "Pat Sample".into());
        let start = std::time::Instant::now();
        let laid = crate::layout(&as_sent(t, &all), &f, "1065|Entity", None);
        let took = start.elapsed();
        let pages = laid.blocks.iter().map(|b| b.page).max().unwrap();
        eprintln!(
            "{} bytes, signature on page {pages}, {took:?}",
            laid.pdf.len()
        );
        assert!(laid.pdf.len() < 2_000_000);
        assert!(pages > 10 && pages < 60);
        assert!(took < std::time::Duration::from_secs(5));
    }

    #[test]
    fn every_sample_lays_out_for_every_return_type() {
        let mut f = crate::Fields::new();
        f.insert("Taxpayer name".into(), "Jordan Sample".into());
        f.insert("Spouse name".into(), "Casey Sample".into());
        f.insert("Signer name".into(), "Jordan Sample".into());
        let all = samples();
        for t in all.iter().filter(|t| t.signed()) {
            for rt in [
                "1040|Individual",
                "1120|Entity",
                "1120-S|Entity",
                "1065|Entity",
                "Individual|Bookkeeping|Payroll|Advisory|FBAR",
                "Entity|Bookkeeping|Payroll|Advisory|FBAR",
            ] {
                let first = rt.split('|').find(|r| *r != "Individual" && *r != "Entity");
                if !rt.split('|').any(|r| t.offered_for(r))
                    && first.is_some_and(|r| !t.offered_for(r))
                {
                    continue;
                }
                let laid = crate::layout(&as_sent(t, &all), &f, rt, t.package.as_deref());
                assert!(
                    !laid.blocks.is_empty(),
                    "{} for {rt} has no signature",
                    t.id
                );
            }
        }
    }
}
