//! Whether two names are one client's (review of PA-94, owner 2026-11-17: "I know there are
//! discrepancies"). The firm's list and the cabinet's folders write one client many ways
//! (`&` and `and`, `Inc` and `Incorporated`, a leading `The`, a middle initial, the words in
//! another order, an ID or `(deceased)` in the folder's name), and two clients almost alike
//! (`John` and `Jon`, `Johnson` and `Johnston`, `Trust` and `Trust II`, `Properties 1` and
//! `Properties 2`). Comparing letter by letter cannot tell these apart at any percentage: at
//! 97 % it missed 17 of 25 of the first kind and took 7 of 11 of the second. Names are
//! compared here word by word, by what each word is.
//!
//! A wrong match renames a folder and changes an ID; a miss costs a click. So the answer is
//! a [`Tier`] with the score:
//! - [`Tier::Exact`]: the same name apart from capitals, punctuation and "Last, First";
//! - [`Tier::Rule`]: the same words once what carries no meaning is set aside (legal forms,
//!   `the`, `and`, a middle initial on one side, word order when one side has no comma);
//! - [`Tier::Near`]: close, with something a person must look at (a number, `Jr` or `II`
//!   on one side, two legal forms, a word more, a word shortened or spelt a letter off).
//!
//! `similar::score` (Similar clients) and Filer's `name_score` (Intake) are not changed.

use std::collections::BTreeSet;

use filer_core::clients::{loose_id, normalize_name};

/// How sure a match is. Ordered: the surest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Exact,
    Rule,
    Near,
}

/// The score every [`Tier::Rule`] match has; a [`Tier::Near`] one is never above
/// [`NEAR_MAX`], an exact one is 1.
pub const RULE_SCORE: f64 = 0.98;
pub const NEAR_MAX: f64 = 0.94;

/// A name taken apart for comparing.
#[derive(Debug, Clone, Default)]
pub struct Name {
    /// The name as plain normalizing leaves it, and "First Last" for "Last, First".
    plain: Vec<String>,
    /// Written "Last, First".
    comma: bool,
    /// The words that carry the name, in order; `last` of them are before the comma.
    core: Vec<String>,
    last: usize,
    initials: Vec<char>,
    numbers: BTreeSet<String>,
    /// `jr`, `sr`, `ii`, `iii` …
    gens: BTreeSet<String>,
    /// Legal forms, in one spelling (`inc`, `llc`, `corp` …).
    forms: BTreeSet<&'static str>,
    /// Its words run together (or, with none, its initials): `Mc Donald` is `mcdonald`.
    joined: String,
}

fn form_of(t: &str) -> Option<&'static str> {
    Some(match t {
        "llc" => "llc",
        "inc" | "incorporated" => "inc",
        "corp" | "corporation" => "corp",
        "co" | "company" => "co",
        "ltd" | "limited" => "ltd",
        "lp" => "lp",
        "llp" => "llp",
        "lllp" => "lllp",
        "pc" => "pc",
        "pllc" => "pllc",
        "pa" => "pa",
        "plc" => "plc",
        "partnership" | "ptnr" | "ptr" => "partnership",
        _ => return None,
    })
}

fn is_noise(t: &str) -> bool {
    matches!(
        t,
        "the" | "and" | "of" | "dba" | "deceased" | "decd" | "mr" | "mrs" | "ms" | "dr"
    )
}

fn is_gen(t: &str) -> bool {
    matches!(t, "jr" | "sr" | "ii" | "iii" | "iv" | "2nd" | "3rd" | "4th")
}

/// One spelling for words written two ways.
fn word(t: &str) -> &str {
    match t {
        "saint" | "street" => "st",
        "avenue" => "ave",
        "road" => "rd",
        "mount" => "mt",
        "brothers" => "bros",
        t => t,
    }
}

impl Name {
    /// `name` taken apart. `id`: the client's ID when it is known; written into the name
    /// (a folder named `Smith, John 1234`), it is not part of the name.
    pub fn of(name: &str, id: Option<&str>) -> Name {
        let mut n = Name::default();
        let plain = normalize_name(name);
        if !plain.is_empty() {
            n.plain.push(plain);
        }
        if let Some((last, first)) = name.split_once(',') {
            let swapped = normalize_name(&format!("{first} {last}"));
            if !swapped.is_empty() && !n.plain.contains(&swapped) {
                n.plain.push(swapped);
            }
        }
        // What is in brackets is about the client, not its name: `(1234)`, `(deceased)`.
        let mut text = String::with_capacity(name.len());
        let mut depth = 0usize;
        for c in name.chars() {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                '&' | '+' if depth == 0 => text.push_str(" and "),
                c if depth == 0 => text.push(c),
                _ => {}
            }
        }
        let id = id.map(loose_id).filter(|i| !i.is_empty());
        let (before, after) = match text.split_once(',') {
            Some((l, r)) => {
                n.comma = true;
                (l.to_string(), r.to_string())
            }
            None => (String::new(), text),
        };
        let mut dated = false;
        for (part, is_last) in [(before, true), (after, false)] {
            for t in normalize_name(&part).split(' ').filter(|t| !t.is_empty()) {
                // A trust's date is not its name: `dtd 1/1/2010` and all after it.
                if matches!(t, "dtd" | "dated" | "uad" | "uta") {
                    dated = true;
                }
                if dated || is_noise(t) || id.as_deref() == Some(loose_id(t).as_str()) {
                    continue;
                }
                if let Some(f) = form_of(t) {
                    n.forms.insert(f);
                } else if is_gen(t) {
                    n.gens.insert(t.to_string());
                } else if t.chars().all(|c| c.is_ascii_digit()) {
                    n.numbers.insert(t.trim_start_matches('0').to_string());
                } else if t.chars().count() == 1 {
                    n.initials.extend(t.chars());
                } else {
                    n.core.push(word(t).to_string());
                    if is_last {
                        n.last += 1;
                    }
                }
            }
        }
        n.joined = n.core.concat();
        if n.joined.is_empty() {
            n.joined = n.initials.iter().collect();
        }
        n
    }

    /// The words to look this name up by (an index over many names): its words, and all of
    /// them run together (`Mc Donald` and `McDonald`, `A & B`).
    pub fn lookup_keys(&self) -> Vec<String> {
        let mut keys = self.core.clone();
        if !self.joined.is_empty() {
            keys.push(self.joined.clone());
        }
        keys.sort();
        keys.dedup();
        keys
    }

    pub fn is_empty(&self) -> bool {
        self.plain.is_empty()
    }
}

/// How well two words go together: 1 the same; less for one shortened to the other's start
/// (`Emp`, `Empl`) or a letter off in a long word (`Johnson`, `Johnston`); 0 otherwise.
fn word_score(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.chars().count() >= 3 && long.starts_with(short) {
        return 0.8;
    }
    if short.chars().count() >= 5 && strsim::jaro_winkler(a, b) >= 0.92 {
        return 0.7;
    }
    0.0
}

/// Pairs the words of `a` with those of `b`, the best pairs first. Returns the sum of the
/// pairs' scores and which words of each were left over.
fn pair_words(a: &[String], b: &[String]) -> (f64, Vec<usize>, Vec<usize>) {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            let s = word_score(x, y);
            if s > 0.0 {
                pairs.push((s, i, j));
            }
        }
    }
    pairs.sort_by(|x, y| y.0.total_cmp(&x.0));
    let (mut used_a, mut used_b) = (vec![false; a.len()], vec![false; b.len()]);
    let mut sum = 0.0;
    for (s, i, j) in pairs {
        if !used_a[i] && !used_b[j] {
            used_a[i] = true;
            used_b[j] = true;
            sum += s;
        }
    }
    let left = |used: &[bool]| -> Vec<usize> {
        used.iter()
            .enumerate()
            .filter(|(_, u)| !**u)
            .map(|(i, _)| i)
            .collect()
    };
    (sum, left(&used_a), left(&used_b))
}

/// How alike two names are (0 to 1) and how sure that is. `None`: nothing in common.
pub fn compare(a: &Name, b: &Name) -> Option<(f64, Tier)> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    // The same as written; or, when only one is written "Last, First", the same once that
    // one is turned round. (Both written so and each the other turned round, `James, Thomas`
    // and `Thomas, James`, are two people.)
    let same = if a.comma == b.comma {
        a.plain[0] == b.plain[0]
    } else {
        a.plain.iter().any(|x| b.plain.contains(x))
    };
    if same {
        return Some((1.0, Tier::Exact));
    }
    // What a person must look at, as the most the score can be.
    let mut cap: f64 = 1.0;
    // Two numbers, two legal forms, `Jr` against `Sr`: two clients.
    if !a.numbers.is_empty() && !b.numbers.is_empty() && a.numbers != b.numbers {
        cap = cap.min(0.5);
    } else if a.numbers != b.numbers {
        cap = cap.min(0.90);
    }
    if !a.gens.is_empty() && !b.gens.is_empty() && a.gens != b.gens {
        cap = cap.min(0.6);
    } else if a.gens != b.gens {
        cap = cap.min(0.90);
    }
    if !a.forms.is_empty() && !b.forms.is_empty() && a.forms != b.forms {
        cap = cap.min(0.90);
    }
    // Words: the same run together (`Mc Donald`, `McDonald`) is every word matched.
    let (mut sum, mut left_a, mut left_b) = if !a.joined.is_empty() && a.joined == b.joined {
        (a.core.len().max(b.core.len()) as f64, vec![], vec![])
    } else {
        pair_words(&a.core, &b.core)
    };
    let whole = sum.fract() == 0.0;
    // "Last, First" on both sides: the family names must be each other's. `James, Thomas`
    // and `Thomas, James` are two people.
    if a.comma && b.comma {
        let (la, lb) = (&a.core[..a.last], &b.core[..b.last]);
        let (s, left_a, left_b) = pair_words(la, lb);
        let apart = !left_a.is_empty() || !left_b.is_empty() || s.fract() != 0.0;
        // (`Mc Donald` and `McDonald` are one family name.)
        if apart && la.concat() != lb.concat() {
            cap = cap.min(0.6);
        }
    }
    // A middle initial on one side and the name in full on the other: `John A`, `John Allen`.
    let mut ia = a.initials.clone();
    let mut ib = b.initials.clone();
    let mut by_initial = |left: &mut Vec<usize>, words: &[String], initials: &mut Vec<char>| {
        left.retain(|&i| {
            let first = words[i].chars().next();
            match initials.iter().position(|c| Some(*c) == first) {
                Some(p) => {
                    initials.remove(p);
                    sum += 1.0;
                    false
                }
                None => true,
            }
        });
    };
    by_initial(&mut left_a, &a.core, &mut ib);
    by_initial(&mut left_b, &b.core, &mut ia);
    // Initials on both sides that are not each other's: `John A`, `John B`.
    for c in ia.clone() {
        if let Some(p) = ib.iter().position(|x| *x == c) {
            ib.remove(p);
            ia.remove(ia.iter().position(|x| *x == c).expect("is there"));
        }
    }
    if !ia.is_empty() && !ib.is_empty() {
        cap = cap.min(0.6);
    }
    let words = a.core.len().max(b.core.len());
    let base = if words == 0 {
        // Nothing but initials (`A & B`): the same letters or nothing.
        if ia.is_empty() && ib.is_empty() && !a.initials.is_empty() {
            1.0
        } else {
            0.0
        }
    } else {
        sum / words as f64
    };
    if base <= 0.0 {
        return None;
    }
    if base >= 1.0 && whole && left_a.is_empty() && left_b.is_empty() && cap >= 1.0 {
        return Some((RULE_SCORE, Tier::Rule));
    }
    Some((base.min(cap).min(NEAR_MAX), Tier::Near))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(a: &str, b: &str) -> Option<(f64, Tier)> {
        let x = compare(&Name::of(a, None), &Name::of(b, None));
        // The same whichever is asked first.
        let y = compare(&Name::of(b, None), &Name::of(a, None));
        assert_eq!(x.map(|v| v.1), y.map(|v| v.1), "{a} / {b}");
        x
    }

    /// One client written two ways is found, and surely enough to be offered as a match.
    #[test]
    fn one_client_written_two_ways_is_a_match() {
        let exact = [
            ("Smith, John & Mary", "John & Mary Smith"),
            ("Smith, John", "John Smith"),
            ("SMITH, JOHN", "Smith, John"),
            ("Acme Holdings LLC", "Acme Holdings, L.L.C."),
            ("O'Brien Plumbing", "OBrien Plumbing"),
            ("Smith-Jones, Ann", "Smith Jones, Ann"),
            ("A & B Trucking", "A&B Trucking"),
        ];
        for (a, b) in exact {
            assert_eq!(cmp(a, b), Some((1.0, Tier::Exact)), "{a} / {b}");
        }
        let rule = [
            ("Smith, John & Mary", "Smith, John and Mary"),
            ("Smith John", "John Smith"),
            ("Smith, John A", "Smith, John"),
            ("Smith, John A.", "Smith, John Allen"),
            ("Acme Holdings LLC", "Acme Holdings"),
            ("Acme Holdings Inc", "Acme Holdings Incorporated"),
            ("Acme Holdings Co", "Acme Holdings Company"),
            ("The Dining Hall LLC", "Dining Hall LLC"),
            ("St. Mary's Church", "Saint Marys Church"),
            ("Smith Family Trust", "Smith Family Trust dtd 1/1/2010"),
            ("Smith, John (deceased)", "Smith, John"),
            ("Smith, John (1234)", "Smith, John"),
            ("Jones Farms Partnership", "Jones Farms"),
            ("Mc Donald, Ann", "McDonald, Ann"),
            ("A and B Trucking", "A & B Trucking"),
        ];
        for (a, b) in rule {
            assert_eq!(cmp(a, b), Some((RULE_SCORE, Tier::Rule)), "{a} / {b}");
        }
    }

    /// Two clients almost alike are never a sure match: at most "close, look", and the
    /// plainly different ones far below.
    #[test]
    fn two_clients_almost_alike_are_never_a_sure_match() {
        let look = [
            ("Smith, John Jr", "Smith, John"),
            ("Smith Family Trust", "Smith Family Trust II"),
            ("Smith Farms LLC", "Smith Farms Inc"),
            ("Acme Holdings LLC", "Acme Holdings II LLC"),
            ("Smith, John - 1040", "Smith, John"),
            ("Johnson, Mark", "Johnston, Mark"),
            (
                "Monett Area Extended Empl Workshop, Inc",
                "Monett Area Extended Emp Workshop",
            ),
            ("Smith, John", "Smith, John & Mary"),
            ("Estate of John Smith", "John Smith"),
        ];
        for (a, b) in look {
            let (s, t) = cmp(a, b).unwrap_or((0.0, Tier::Near));
            assert_eq!(t, Tier::Near, "{a} / {b}");
            assert!(s <= NEAR_MAX, "{a} / {b}: {s}");
        }
        let different = [
            ("Smith, John", "Smith, Joan"),
            ("Smith, John", "Smith, Jon"),
            ("Smith, John A", "Smith, John B"),
            ("Miller, David", "Miller, Daniel"),
            ("123 Main Street LLC", "125 Main Street LLC"),
            ("Baker Properties 1 LLC", "Baker Properties 2 LLC"),
            ("James, Thomas", "Thomas, James"),
            ("Smith, John Jr", "Smith, John Sr"),
        ];
        for (a, b) in different {
            let (s, t) = cmp(a, b).unwrap_or((0.0, Tier::Near));
            assert_eq!(t, Tier::Near, "{a} / {b}");
            assert!(s <= 0.6, "{a} / {b}: {s}");
        }
        assert_eq!(cmp("Acme Holdings LLC", "Zeta Farms Inc"), None);
    }

    /// The client's ID written into a folder's name is not part of the name; another
    /// number is.
    #[test]
    fn the_clients_own_id_in_its_name_is_set_aside() {
        let list = Name::of("Smith, John", None);
        let folder = Name::of("Smith, John 01234", Some("1234-0"));
        assert_eq!(compare(&list, &folder).unwrap().1, Tier::Near);
        let folder = Name::of("Smith, John 1234", Some("12-34"));
        assert_eq!(compare(&list, &folder), Some((RULE_SCORE, Tier::Rule)));
    }
}
