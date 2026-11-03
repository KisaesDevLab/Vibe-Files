//! Which client a recognized form belongs to (PHASES P08 step 3).
//!
//! A client matches when the form's TIN last 4 **and** its name agree with the client list
//! (last 4 alone collides at a few hundred clients). A learned alias (payer name → client,
//! from corrections at filing, step 4) counts like the name. Name alone, or last 4 alone,
//! stays below [`THRESHOLD`]: the file then reaches Intake unmatched, never auto-confirmed.
//! For a K-1 both parties may be clients: the entity is the primary destination and the
//! partner / shareholder / beneficiary a copy (Filer's rule copies take it from there).

use crate::forms::{Extract, Form};

/// Proposals at or above this go to Intake with the client's ID in the file name.
pub const THRESHOLD: f32 = 0.85;

/// A client as the matcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRef {
    /// Catalog / sentinel uuid.
    pub id: String,
    /// CSV / tax-software ID (what goes into the file name).
    pub ext_id: String,
    pub name: String,
    pub tin4: Option<String>,
}

/// A learned correction: forms from `payer` belong to client `client_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub payer: String,
    pub client_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub client_id: String,
    pub ext_id: String,
    pub name: String,
    pub confidence: f32,
    /// Why, for the Intake note: "TIN and name", "name only", …
    pub why: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub form: Form,
    pub doc_type: &'static str,
    pub tax_year: Option<i32>,
    /// Where it files (`None`: no client matched).
    pub primary: Option<Match>,
    /// Other clients that get a copy (a K-1's partner when the entity is also a client).
    pub copies: Vec<Match>,
}

impl Proposal {
    /// Whether the primary destination is sure enough to name the file for it.
    pub fn confident(&self) -> bool {
        self.primary
            .as_ref()
            .is_some_and(|m| m.confidence >= THRESHOLD)
    }
}

const NOISE: [&str; 14] = [
    "LLC", "INC", "CORP", "CO", "LP", "LLP", "PC", "PLLC", "THE", "AND", "OF", "NA", "LTD",
    "COMPANY",
];

fn tokens(name: &str) -> Vec<String> {
    let mut v: Vec<String> = name
        .to_uppercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !NOISE.contains(t))
        .map(str::to_string)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// 0..1: the share of the shorter name's words found in the other (`Reyes, Dana & Lee` and
/// `DANA REYES` → 1.0).
pub fn name_score(a: &str, b: &str) -> f32 {
    let (ta, tb) = (tokens(a), tokens(b));
    let n = ta.len().min(tb.len());
    if n == 0 {
        return 0.0;
    }
    let shared = ta.iter().filter(|t| tb.contains(t)).count();
    // One shared word between two multi-word names is weak (first names repeat).
    if shared == 1 && n > 1 {
        return 0.4;
    }
    shared as f32 / n as f32
}

fn norm(s: &str) -> String {
    tokens(s).join(" ")
}

/// The best client for one party of a form.
fn best(
    name: Option<&str>,
    tin4: Option<&str>,
    payer: Option<&str>,
    clients: &[ClientRef],
    aliases: &[Alias],
) -> Option<Match> {
    let alias_for = payer.and_then(|p| {
        let p = norm(p);
        aliases
            .iter()
            .find(|a| norm(&a.payer) == p)
            .map(|a| a.client_id.as_str())
    });
    let mut scored: Vec<(f32, &'static str, &ClientRef)> = clients
        .iter()
        .map(|c| {
            let tin = match (tin4, c.tin4.as_deref()) {
                (Some(a), Some(b)) if a == b => Some(true),
                (Some(_), Some(_)) => Some(false),
                _ => None,
            };
            let ns = name.map_or(0.0, |n| name_score(n, &c.name));
            let alias = alias_for == Some(c.id.as_str());
            let (score, why) = match (tin, ns >= 0.6, alias) {
                (Some(false), _, _) => (0.0, "TIN differs"),
                (Some(true), true, _) => (0.97, "TIN and name"),
                (Some(true), false, true) => (0.95, "TIN and learned payer"),
                (None, true, true) => (0.9, "name and learned payer"),
                (None, false, true) => (0.86, "learned payer"),
                (Some(true), false, false) => (0.6, "TIN only"),
                (None, true, false) if ns >= 0.99 => (0.8, "name only"),
                (None, true, false) => (0.65, "name only"),
                (None, false, false) => (0.0, ""),
            };
            (score, why, c)
        })
        .filter(|(s, _, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (score, why, c) = scored.first()?;
    // Two clients equally likely: not sure.
    let tie = scored.get(1).is_some_and(|s| (s.0 - score).abs() < 0.01);
    Some(Match {
        client_id: c.id.clone(),
        ext_id: c.ext_id.clone(),
        name: c.name.clone(),
        confidence: if tie { score.min(0.7) } else { *score },
        why: if tie {
            "more than one client fits"
        } else {
            why
        },
    })
}

/// The proposal for a recognized form.
pub fn propose(e: &Extract, clients: &[ClientRef], aliases: &[Alias]) -> Proposal {
    let tin = |t: &Option<crate::forms::Last4>| t.as_ref().map(|l| l.as_str().to_string());
    let recipient = best(
        e.recipient_name.as_deref(),
        tin(&e.recipient_tin4).as_deref(),
        e.payer_name.as_deref(),
        clients,
        aliases,
    );
    let (primary, copies) = if e.form.is_k1() {
        // The entity issuing the K-1, when it is a client (no alias: that is about payers of
        // the client side).
        let entity = best(
            e.payer_name.as_deref(),
            tin(&e.payer_tin4).as_deref(),
            None,
            clients,
            &[],
        );
        // The entity leads only when it is a sure match itself (a last-4 collision with another
        // client must not take the K-1 away from its partner).
        let entity = entity.filter(|en| en.confidence >= THRESHOLD);
        match (entity, recipient) {
            (Some(en), Some(r)) if en.client_id != r.client_id && r.confidence >= THRESHOLD => {
                (Some(en), vec![r])
            }
            (Some(en), _) => (Some(en), vec![]),
            (None, r) => (r, vec![]),
        }
    } else {
        (recipient, vec![])
    };
    Proposal {
        form: e.form,
        doc_type: e.form.doc_type(),
        tax_year: e.tax_year,
        primary,
        copies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_by_words() {
        assert_eq!(name_score("Reyes, Dana & Lee", "DANA REYES"), 1.0);
        assert_eq!(
            name_score("Harbor Point Farms LLC", "Harbor Point Farms"),
            1.0
        );
        assert!(name_score("Dana Reyes", "Dana Smith") < 0.6);
        assert_eq!(name_score("", "x"), 0.0);
    }
}
