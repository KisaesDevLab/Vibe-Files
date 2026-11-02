//! The rule builder (V10): words picked from a test file name, a guided form, and the regex the
//! form stands for. The regex crate has no look-around, so "contains all of" is written as every
//! order of the words (up to four) and "does not contain" becomes the rule's `exclude` pattern.

use crate::rules::{END, START};

/// What the user asked for. Words are matched whole, any case; a space also matches `_` and `-`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub any_of: Vec<String>,
    pub all_of: Vec<String>,
    pub none_of: Vec<String>,
    pub starts_with: String,
    pub ends_with: String,
    /// A 4-digit year in a word matches any year (`\d{4}`).
    pub any_year: bool,
    /// Any run of digits in a word matches any number (`\d+`).
    pub any_number: bool,
}

/// The regexes for a rule: `pattern` must match, `exclude` must not (empty: none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub pattern: String,
    pub exclude: String,
}

/// Up to this many "all of" parts get every order; more are kept in the order given.
const MAX_ORDERS: usize = 4;

/// The words of a file name's remainder, as chips: split on spaces and `_`, outer punctuation
/// trimmed ("(2)" → "2", "Smith," → "Smith"). Hyphenated words like "W-2" stay whole.
pub fn words_in(rest: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in rest.split(|c: char| c.is_whitespace() || c == '_') {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        if !w.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(w)) {
            out.push(w.to_string());
        }
    }
    out
}

impl Form {
    pub fn is_empty(&self) -> bool {
        self.any_of.is_empty()
            && self.all_of.is_empty()
            && self.starts_with.trim().is_empty()
            && self.ends_with.trim().is_empty()
    }

    /// The pattern (and exclude) this form stands for.
    pub fn build(&self) -> Result<Built, String> {
        if self.is_empty() {
            return Err(
                "Click words in the test name, or type them, to say what the rule matches.".into(),
            );
        }
        let clean = |list: &[String]| -> Vec<String> {
            list.iter()
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty())
                .collect()
        };
        let mut elements: Vec<El> = clean(&self.all_of).iter().map(|w| self.el(w)).collect();
        let any: Vec<El> = clean(&self.any_of).iter().map(|w| self.el(w)).collect();
        if let Some(group) = El::any(&any) {
            elements.push(group);
        }
        let orders = if elements.len() > 1 && elements.len() <= MAX_ORDERS {
            permutations(&elements)
        } else {
            vec![elements]
        };
        let start = self.starts_with.trim();
        let end = self.ends_with.trim();
        let rendered: Vec<String> = orders
            .into_iter()
            .map(|mut order| {
                let mut out = String::new();
                if !start.is_empty() {
                    out.push('^');
                    order.insert(
                        0,
                        El {
                            sa: false,
                            ..self.el(start)
                        },
                    );
                }
                if !end.is_empty() {
                    order.push(El {
                        ea: false,
                        ..self.el(end)
                    });
                }
                if order.first().is_some_and(|e| e.sa) {
                    out.push_str(START);
                }
                for (i, e) in order.iter().enumerate() {
                    if i > 0 {
                        out.push_str(sep(order[i - 1].ea, e.sa));
                    }
                    out.push_str(&e.core);
                }
                if !end.is_empty() {
                    out.push('$');
                } else if order.last().is_some_and(|e| e.ea) {
                    out.push_str(END);
                }
                out
            })
            .collect();
        let pattern = if rendered.len() == 1 {
            rendered[0].clone()
        } else {
            format!("(?:{})", rendered.join("|"))
        };
        let none: Vec<String> = clean(&self.none_of)
            .iter()
            .map(|w| self.el(w).with_edges())
            .collect();
        let exclude = match none.len() {
            0 => String::new(),
            1 => none[0].clone(),
            _ => format!("(?:{})", none.join("|")),
        };
        Ok(Built { pattern, exclude })
    }

    /// One word or phrase as a regex piece: escaped, spaces matching `_` and `-` too, digits
    /// generalized if asked. Its whole-word edges are added where it meets its neighbours.
    fn el(&self, w: &str) -> El {
        let chars: Vec<char> = w.chars().collect();
        let mut core = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                let j = (i..chars.len())
                    .find(|&k| !chars[k].is_ascii_digit())
                    .unwrap_or(chars.len());
                let run: String = chars[i..j].iter().collect();
                let year = run.len() == 4 && (run.starts_with("19") || run.starts_with("20"));
                if self.any_year && year {
                    core.push_str(r"\d{4}");
                } else if self.any_number {
                    core.push_str(r"\d+");
                } else {
                    core.push_str(&run);
                }
                i = j;
            } else if c.is_whitespace() {
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                core.push_str(r"[\s_-]+");
            } else {
                core.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
        }
        El {
            core,
            sa: chars.first().is_some_and(|c| c.is_alphanumeric()),
            ea: chars.last().is_some_and(|c| c.is_alphanumeric()),
        }
    }

    /// A rule name from the picked words: "W-2", "K-1 package"…
    pub fn suggested_name(&self) -> String {
        self.all_of
            .iter()
            .chain(&self.any_of)
            .chain(std::iter::once(&self.starts_with))
            .find(|w| !w.trim().is_empty())
            .map(|w| w.trim().to_string())
            .unwrap_or_default()
    }
}

/// A word as a regex piece, and whether it starts / ends with a letter or digit (then it needs a
/// separator or the text's edge on that side to count as a whole word).
#[derive(Debug, Clone)]
struct El {
    core: String,
    sa: bool,
    ea: bool,
}

impl El {
    fn with_edges(&self) -> String {
        format!(
            "{}{}{}",
            if self.sa { START } else { "" },
            self.core,
            if self.ea { END } else { "" }
        )
    }

    /// "Any of" as one piece. Words with the same kind of edges share them; otherwise each
    /// keeps its own (then a neighbour right next to it needs its own separator).
    fn any(items: &[El]) -> Option<El> {
        match items {
            [] => None,
            [one] => Some(one.clone()),
            _ if items
                .iter()
                .all(|e| e.sa == items[0].sa && e.ea == items[0].ea) =>
            {
                Some(El {
                    core: format!(
                        "(?:{})",
                        items
                            .iter()
                            .map(|e| e.core.as_str())
                            .collect::<Vec<_>>()
                            .join("|")
                    ),
                    sa: items[0].sa,
                    ea: items[0].ea,
                })
            }
            _ => Some(El {
                core: format!(
                    "(?:{})",
                    items
                        .iter()
                        .map(El::with_edges)
                        .collect::<Vec<_>>()
                        .join("|")
                ),
                sa: false,
                ea: false,
            }),
        }
    }
}

/// Between two pieces: anything, but a word piece must have a separator on its side. One
/// separator serves both (the edges can't share a character otherwise, as they consume it).
fn sep(left_word: bool, right_word: bool) -> &'static str {
    match (left_word, right_word) {
        (true, true) => "[^a-z0-9](?:.*[^a-z0-9])?",
        (true, false) => "(?:[^a-z0-9].*)?",
        (false, true) => "(?:.*[^a-z0-9])?",
        (false, false) => ".*",
    }
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(i);
        for mut p in permutations(&rest) {
            p.insert(0, first.clone());
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::compile_pattern;

    fn matches(b: &Built, rest: &str) -> bool {
        compile_pattern(&b.pattern).unwrap().is_match(rest)
            && (b.exclude.is_empty() || !compile_pattern(&b.exclude).unwrap().is_match(rest))
    }

    #[test]
    fn words_come_from_the_rest() {
        assert_eq!(
            words_in("2025 W-2 (corrected)_Smith, John"),
            ["2025", "W-2", "corrected", "Smith", "John"]
        );
        assert_eq!(words_in("a A a"), ["a"]);
    }

    #[test]
    fn any_all_none_start_end() {
        let f = Form {
            any_of: vec!["W-2".into(), "1099".into()],
            ..Default::default()
        };
        let b = f.build().unwrap();
        assert!(matches(&b, "2025 W-2 corrected"));
        assert!(matches(&b, "1099-INT"));
        assert!(!matches(&b, "W-20"), "whole words");
        assert!(!matches(&b, "10990"));

        let f = Form {
            all_of: vec!["fixed assets".into(), "2025".into()],
            any_year: true,
            none_of: vec!["draft".into()],
            ..Default::default()
        };
        let b = f.build().unwrap();
        assert!(matches(&b, "Fixed_Assets 2024"), "spaces match _, any year");
        assert!(matches(&b, "2023 fixed-assets"), "either order");
        assert!(!matches(&b, "Fixed Assets"), "both words needed");
        assert!(!matches(&b, "Fixed Assets 2024 draft"), "excluded");

        let f = Form {
            starts_with: "Scan".into(),
            ends_with: "signed".into(),
            ..Default::default()
        };
        let b = f.build().unwrap();
        assert!(matches(&b, "Scan 12 signed"));
        assert!(!matches(&b, "x Scan 12 signed"));
        assert!(!matches(&b, "Scan 12 signed copy"));
        assert_eq!(f.suggested_name(), "Scan");

        let f = Form {
            any_of: vec!["inv 123".into()],
            any_number: true,
            ..Default::default()
        };
        assert!(matches(&f.build().unwrap(), "INV_98765"));
        assert!(Form::default().build().is_err());
    }
}
