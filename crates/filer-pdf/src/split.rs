//! Proposing where to cut a multi-client PDF: find a client on each page (an ID from the client
//! list, else a client's name as whole words, else a close name match on one of the first lines)
//! and start a new piece where the client changes. Pages before the first page that names a
//! client join that client's piece (X95). No text at all gives one piece without a client.

use std::collections::BTreeSet;
use std::ops::Range;

use regex::Regex;

use crate::clients::{ClientList, normalize_name};

/// How a page's client was found.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    Id,
    Name,
    Similar(f64),
}

/// The client found on one page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageClient {
    pub client_id: String,
    pub found: Found,
    /// 1-based line of the page text where it was found, when it is on one line.
    pub line: Option<usize>,
}

/// One piece of the split.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// 0-based page indexes, end exclusive.
    pub pages: Range<usize>,
    pub client_id: Option<String>,
    /// First page in the piece where its client was found, and how.
    pub found_on: Option<(usize, Found)>,
}

/// 4-digit numbers this many years back (to next year) are taken as years on a page, not IDs.
const PAGE_YEARS_BACK: i32 = 5;
/// Lines of a page tried for a similar-name match.
const NAME_LINES: usize = 15;
/// Names shorter than this (normalized) are not looked for as text, to avoid false hits.
const MIN_NAME_LEN: usize = 4;

/// Client names prepared for searching page text.
pub struct Finder<'a> {
    clients: &'a ClientList,
    id_re: Regex,
    names: Vec<(String, Vec<String>)>,
    threshold: f64,
    current_year: i32,
}

impl<'a> Finder<'a> {
    pub fn new(
        clients: &'a ClientList,
        id_pattern: &str,
        threshold: f32,
        current_year: i32,
    ) -> Result<Self, String> {
        // Just the ID pattern; the characters around a match are checked without being consumed,
        // so "2025 1002" still finds 1002 after skipping 2025.
        let id_re = Regex::new(id_pattern)
            .map_err(|e| format!("the client ID pattern is not valid: {e}"))?;
        let names = clients
            .clients
            .iter()
            .map(|c| {
                let mut v = vec![normalize_name(&c.name)];
                if let Some((last, first)) = c.name.split_once(',') {
                    v.push(normalize_name(&format!("{first} {last}")));
                }
                v.retain(|n| n.chars().count() >= MIN_NAME_LEN);
                (c.id.clone(), v)
            })
            .collect();
        Ok(Self {
            clients,
            id_re,
            names,
            threshold: f64::from(threshold),
            current_year,
        })
    }

    /// The client named on a page, if any.
    pub fn page_client(&self, text: &str) -> Option<PageClient> {
        let bytes = text.as_bytes();
        let isolated = |start: usize, end: usize| {
            let before = start.checked_sub(1).map(|i| bytes[i]);
            let after = bytes.get(end).copied();
            !before.is_some_and(|b| b.is_ascii_alphanumeric())
                && !after.is_some_and(|b| b.is_ascii_alphanumeric())
        };
        for m in self.id_re.find_iter(text) {
            if !isolated(m.start(), m.end()) {
                continue;
            }
            let id = m.as_str();
            // Years are on nearly every tax page; a client ID that looks like a recent one is only
            // found by name. A narrower window than file names use (X144), so IDs like 2018 still
            // count.
            if id.len() == 4
                && id.parse::<i32>().is_ok_and(|y| {
                    (self.current_year - PAGE_YEARS_BACK..=self.current_year + 1).contains(&y)
                })
            {
                continue;
            }
            if let Some(client) = self.clients.match_by_id(id) {
                return Some(PageClient {
                    client_id: client.id.clone(),
                    found: Found::Id,
                    line: Some(text[..m.start()].matches('\n').count() + 1),
                });
            }
        }
        let haystack = format!(" {} ", normalize_name(text));
        let mut best: Option<(&str, &str)> = None;
        for (id, variants) in &self.names {
            for v in variants {
                if v.len() > best.map_or(0, |b| b.1.len()) && haystack.contains(&format!(" {v} ")) {
                    best = Some((id, v));
                }
            }
        }
        if let Some((id, v)) = best {
            let needle = format!(" {v} ");
            let line = text
                .lines()
                .position(|l| format!(" {} ", normalize_name(l)).contains(&needle))
                .map(|i| i + 1);
            return Some(PageClient {
                client_id: id.to_string(),
                found: Found::Name,
                line,
            });
        }
        self.best_similar(text)
            .filter(|(s, ..)| *s >= self.threshold)
            .map(|(score, client_id, line)| PageClient {
                client_id,
                found: Found::Similar(score),
                line: Some(line),
            })
    }

    /// The closest client name on one of the page's first lines, however close:
    /// (score, client ID, 1-based line).
    pub fn best_similar(&self, text: &str) -> Option<(f64, String, usize)> {
        text.lines()
            .enumerate()
            .map(|(i, l)| (i, l.trim()))
            .filter(|(_, l)| (3..=80).contains(&l.chars().count()))
            .take(NAME_LINES)
            .filter_map(|(i, line)| {
                self.clients
                    .fuzzy_by_name(line, 1)
                    .into_iter()
                    .next()
                    .map(|(s, c)| (s, c.id.clone(), i + 1))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }
}

/// Cuts (page indexes that start a new piece) where the detected client changes.
pub fn proposed_cuts(page_clients: &[Option<PageClient>]) -> BTreeSet<usize> {
    let mut cuts = BTreeSet::new();
    let mut current: Option<&str> = None;
    for (i, pc) in page_clients.iter().enumerate() {
        let Some(pc) = pc else { continue };
        match current {
            None => current = Some(&pc.client_id),
            Some(c) if c != pc.client_id => {
                cuts.insert(i);
                current = Some(&pc.client_id);
            }
            Some(_) => {}
        }
    }
    cuts
}

/// The pieces for `total` pages split at `cuts`. Each piece's client is the first one found on
/// its pages.
pub fn segments_from_cuts(
    total: usize,
    cuts: &BTreeSet<usize>,
    page_clients: &[Option<PageClient>],
) -> Vec<Segment> {
    let mut starts: Vec<usize> = std::iter::once(0)
        .chain(cuts.iter().copied().filter(|c| *c > 0 && *c < total))
        .collect();
    starts.dedup();
    let mut out = Vec::new();
    for (k, start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(total);
        let found =
            (*start..end).find_map(|p| page_clients.get(p).cloned().flatten().map(|pc| (p, pc)));
        out.push(Segment {
            pages: *start..end,
            client_id: found.as_ref().map(|(_, pc)| pc.client_id.clone()),
            found_on: found.map(|(p, pc)| (p, pc.found)),
        });
    }
    out
}

/// Detects clients on every page and proposes the pieces.
pub fn propose(texts: &[String], finder: &Finder<'_>) -> (Vec<Option<PageClient>>, Vec<Segment>) {
    let page_clients: Vec<Option<PageClient>> =
        texts.iter().map(|t| finder.page_client(t)).collect();
    let cuts = proposed_cuts(&page_clients);
    let segments = segments_from_cuts(texts.len(), &cuts, &page_clients);
    (page_clients, segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ColumnMap;

    fn clients() -> ClientList {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("c.csv");
        std::fs::write(
            &csv,
            "id,name\n1001,\"Smith, John\"\n1002,Acme LLC\n1003,Acme LLC Holdings\n1004,Baker Farms\n2025,Year Client\n",
        )
        .unwrap();
        let map = ColumnMap {
            id: Some("id".into()),
            name: Some("name".into()),
            extras: vec![],
        };
        crate::clients::apply_mapping(&crate::clients::read_csv(&csv).unwrap(), &map).unwrap()
    }

    fn finder(list: &ClientList) -> Finder<'_> {
        Finder::new(list, crate::parse::DEFAULT_ID_PATTERN, 0.9, 2026).unwrap()
    }

    #[test]
    fn page_client_by_id_name_and_similar_name() {
        let list = clients();
        let f = finder(&list);
        let id = |t: &str| f.page_client(t).map(|p| (p.client_id, p.found));
        assert_eq!(
            id("Client #1002 statement"),
            Some(("1002".into(), Found::Id))
        );
        assert_eq!(
            id("ref 99991002 only"),
            None,
            "digits inside a longer number"
        );
        assert_eq!(
            id("Prepared for JOHN SMITH"),
            Some(("1001".into(), Found::Name))
        );
        assert_eq!(
            id("Acme LLC Holdings annual"),
            Some(("1003".into(), Found::Name)),
            "the longest name wins"
        );
        match id("Organizer\nBaker Farmz\n2025") {
            Some((c, Found::Similar(s))) => {
                assert_eq!(c, "1004");
                assert!(s >= 0.9);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(id("nothing here"), None);
        // "2025" is a year here, not client 2025; the name still finds that client.
        assert_eq!(id("Tax year 2025"), None);
        assert_eq!(
            id("Tax year 2025 1002 Acme"),
            Some(("1002".into(), Found::Id)),
            "after a skipped year"
        );
        assert_eq!(id("ID:1002;"), Some(("1002".into(), Found::Id)));
        assert_eq!(id("Year Client 2025"), Some(("2025".into(), Found::Name)));
    }

    #[test]
    fn twelve_pages_three_clients_three_pieces() {
        let list = clients();
        let texts: Vec<String> = [
            "Cover sheet - print run",
            "Client 1001 - Form 1040",
            "page 2 of return",
            "page 3 of return",
            "Acme LLC 1120-S",
            "schedule K",
            "continued",
            "statement",
            "Baker Farms 1065 return",
            "page 2",
            "page 3",
            "end",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (pcs, segs) = propose(&texts, &finder(&list));
        assert_eq!(pcs.iter().filter(|p| p.is_some()).count(), 3);
        let summary: Vec<(Range<usize>, Option<&str>)> = segs
            .iter()
            .map(|s| (s.pages.clone(), s.client_id.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                (0..4, Some("1001")),
                (4..8, Some("1002")),
                (8..12, Some("1004"))
            ],
            "the cover page joins the first client (X95)"
        );
        assert_eq!(segs[0].found_on, Some((1, Found::Id)));
    }

    #[test]
    fn no_text_is_one_piece_and_manual_cuts_work() {
        let list = clients();
        let texts = vec![String::new(); 6];
        let (pcs, segs) = propose(&texts, &finder(&list));
        assert_eq!(segs.len(), 1);
        assert_eq!(
            (segs[0].pages.clone(), segs[0].client_id.clone()),
            (0..6, None)
        );
        let cuts: BTreeSet<usize> = [2, 4].into();
        let manual = segments_from_cuts(6, &cuts, &pcs);
        let ranges: Vec<Range<usize>> = manual.iter().map(|s| s.pages.clone()).collect();
        assert_eq!(ranges, [0..2, 2..4, 4..6]);
        // Cuts at 0 or past the end are ignored.
        let odd: BTreeSet<usize> = [0, 9].into();
        assert_eq!(segments_from_cuts(6, &odd, &pcs).len(), 1);
    }

    #[test]
    fn same_client_again_later_is_a_new_piece() {
        let list = clients();
        let texts: Vec<String> = ["Client 1001", "Client 1002", "Client 1001"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (_, segs) = propose(&texts, &finder(&list));
        assert_eq!(segs.len(), 3);
    }

    /// A run of Form 9325 acknowledgements, one page per client named "First Last" in capitals:
    /// one piece per client, each filed by the Firm "Form 9325" rule from its page text.
    #[test]
    fn form_9325_print_run_splits_per_client() {
        let list = clients();
        let texts: Vec<String> = [
            "Form 9325 Acknowledgement and General Information
JOHN SMITH
SSN XXX-XX-4321
AR 72701",
            "Form 9325 Acknowledgement and General Information
ACME LLC
EIN XX-XXX9876",
            "Form 9325 Acknowledgement and General Information
BAKER FARMS",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (_, segs) = propose(&texts, &finder(&list));
        let summary: Vec<(Range<usize>, Option<&str>)> = segs
            .iter()
            .map(|s| (s.pages.clone(), s.client_id.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                (0..1, Some("1001")),
                (1..2, Some("1002")),
                (2..3, Some("1004"))
            ]
        );
        let rules = crate::rules::CompiledRules::new(&crate::rules::firm_rules()).unwrap();
        for t in &texts {
            assert_eq!(
                rules.first_specific_match(t, false).unwrap().name,
                "Form 9325"
            );
        }
    }
}
