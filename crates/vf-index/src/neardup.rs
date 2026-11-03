//! Near-duplicate report (P03 step 7): documents whose text is almost the same (a rescan of
//! the same page, a printed and a scanned copy) but whose bytes differ. Word 5-gram
//! shingles, MinHash signatures and banding find candidate pairs; each candidate is then
//! scored by the exact Jaccard similarity of its shingle sets.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use vf_core::db::Connection;

/// Words per shingle.
const K: usize = 5;
/// MinHash signature length = BANDS × ROWS.
const BANDS: usize = 16;
const ROWS: usize = 4;
/// Documents with fewer shingles than this are too short to compare.
const MIN_SHINGLES: usize = 8;

/// Two documents with nearly the same text.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub a: String,
    pub a_path: String,
    pub b: String,
    pub b_path: String,
    /// Jaccard similarity of the shingle sets, 0–1.
    pub similarity: f32,
}

fn h64(v: impl Hash) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut s);
    s.finish()
}

/// The document's shingles (hashed): lower-cased words, punctuation dropped.
pub fn shingles(text: &str) -> HashSet<u64> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect();
    words.windows(K).map(h64).collect()
}

fn signature(sh: &HashSet<u64>) -> [u64; BANDS * ROWS] {
    let mut sig = [u64::MAX; BANDS * ROWS];
    for &s in sh {
        for (i, slot) in sig.iter_mut().enumerate() {
            let v = h64((s, i as u64));
            if v < *slot {
                *slot = v;
            }
        }
    }
    sig
}

fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f32 {
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

/// Pairs of live documents at or above `threshold` (e.g. 0.8), most similar first. Exact
/// duplicates (same content hash) are left out: step 7's intake check covers those.
pub fn report(conn: &Connection, threshold: f32) -> vf_core::Result<Vec<Pair>> {
    let mut st = conn.prepare(
        "SELECT d.id, d.rel_path, d.hash, group_concat(p.text, ' ')
         FROM document d JOIN page_text p ON p.hash = d.hash
         WHERE d.deleted_at IS NULL GROUP BY d.id ORDER BY d.id",
    )?;
    let docs: Vec<(String, String, String, HashSet<u64>)> = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            ))
        })?
        .filter_map(Result::ok)
        .map(|(id, path, hash, text)| (id, path, hash, shingles(&text)))
        .filter(|d| d.3.len() >= MIN_SHINGLES)
        .collect();
    let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    for (i, d) in docs.iter().enumerate() {
        let sig = signature(&d.3);
        for b in 0..BANDS {
            buckets
                .entry((b, h64(&sig[b * ROWS..(b + 1) * ROWS])))
                .or_default()
                .push(i);
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for members in buckets.values().filter(|m| m.len() > 1) {
        for (x, &i) in members.iter().enumerate() {
            for &j in &members[x + 1..] {
                if !seen.insert((i.min(j), i.max(j))) || docs[i].2 == docs[j].2 {
                    continue;
                }
                let s = jaccard(&docs[i].3, &docs[j].3);
                if s >= threshold {
                    out.push(Pair {
                        a: docs[i].0.clone(),
                        a_path: docs[i].1.clone(),
                        b: docs[j].0.clone(),
                        b_path: docs[j].1.clone(),
                        similarity: s,
                    });
                }
            }
        }
    }
    out.sort_by(|p, q| {
        q.similarity
            .total_cmp(&p.similarity)
            .then_with(|| p.a_path.cmp(&q.a_path))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::db::{CatalogKey, Storage};

    const LETTER: &str = "Dear client, thank you for choosing our firm to prepare your 2025 \
        individual income tax return. This letter confirms the scope of the engagement, the \
        documents you agreed to provide, our fees, and the deadline for the return.";

    #[test]
    fn rescans_are_near_duplicates_and_other_letters_are_not() {
        let t = tempfile::tempdir().unwrap();
        let c = vf_core::db::open(&t.path().join("c.db"), &CatalogKey([8; 32]), Storage::Local)
            .unwrap();
        // A rescan: OCR misread two words. A different letter. An exact copy.
        let rescan = LETTER
            .replace("choosing", "chooslng")
            .replace("fees,", "fces,");
        let other = "Payroll summary for the third quarter: gross wages, federal withholding, \
            social security and medicare totals by employee, with the 941 reconciliation \
            attached for review before filing.";
        for (id, hash, text) in [
            ("a", "ha", LETTER),
            ("b", "hb", rescan.as_str()),
            ("c", "hc", other),
            ("d", "ha", LETTER),
        ] {
            c.execute(
                "INSERT INTO document (id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, ?2, ?3, 1, 't', 'dev', 't')",
                (id, format!("{id}.pdf"), hash),
            )
            .unwrap();
            c.execute(
                "INSERT OR IGNORE INTO page_text (hash, page, source, text) VALUES (?1, 0, 'ocr', ?2)",
                (hash, text),
            )
            .unwrap();
        }
        let r = report(&c, 0.5).unwrap();
        let pairs: Vec<(&str, &str)> = r.iter().map(|p| (p.a.as_str(), p.b.as_str())).collect();
        assert!(pairs.contains(&("a", "b")), "{r:?}");
        assert!(pairs.contains(&("b", "d")), "{r:?}");
        assert!(
            !pairs.contains(&("a", "d")),
            "exact duplicates are left out"
        );
        assert!(r.iter().all(|p| p.a != "c" && p.b != "c"));
        assert!(r[0].similarity >= 0.5 && r[0].similarity < 1.0);
        assert!(report(&c, 0.99).unwrap().is_empty());
    }
}
