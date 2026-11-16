//! What this computer knows to be in the bucket, by client: the list Vibe Files sends to the
//! firm's portal (PA-80), so its staff page can show each client's documents in B2. Read from
//! the catalog alone (`sync_base`, the documents as last exchanged): nothing asks the bucket.

use std::collections::{BTreeMap, BTreeSet};

use vf_core::db::Connection;

use crate::Result;
use crate::catalog;

/// A document in the bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedDoc {
    pub doc_id: String,
    /// Below the client's folder, `/` between folders, with the file name.
    pub path: String,
    pub size: i64,
    pub tax_year: Option<i32>,
}

/// A client with a manifest segment here, and its documents in the bucket (none: every one was
/// taken out, or deleted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedClient {
    pub id: String,
    /// The client ID of the client list (`ext_id`).
    pub code: String,
    pub name: String,
    /// The client's segment as last exchanged.
    pub rev: u64,
    pub docs: Vec<ListedDoc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    /// The newest state of the bucket this computer has seen.
    pub head_rev: u64,
    /// By client id; documents by path.
    pub clients: Vec<ListedClient>,
    /// Every client the cabinet has here, with a segment or not.
    pub live: BTreeSet<String>,
}

/// The path below the client's folder, with `/` between folders.
fn below(folder: &str, rel_path: &str) -> String {
    let rel = rel_path.replace('\\', "/");
    let folder = folder.replace('\\', "/");
    let folder = folder.trim_matches('/');
    if folder.is_empty() {
        return rel;
    }
    match rel.get(..folder.len()) {
        Some(head) if head.eq_ignore_ascii_case(folder) => {
            match rel[folder.len()..].strip_prefix('/') {
                Some(rest) if !rest.is_empty() => rest.to_string(),
                _ => rel,
            }
        }
        _ => rel,
    }
}

/// The clients with a segment here and their documents in the bucket. `device`: the id B2 sync
/// uses for this catalog.
pub fn listed(conn: &Connection, device: &str) -> Result<Listed> {
    // One picture of the catalog: a push that ends between two of the reads below would pair
    // an old revision with new documents.
    let tx = conn.unchecked_transaction()?;
    let out = read(&tx, device)?;
    tx.commit()?;
    Ok(out)
}

fn read(conn: &Connection, device: &str) -> Result<Listed> {
    let mut out = Listed {
        head_rev: catalog::last_rev(conn, device)?,
        ..Default::default()
    };
    let mut by_id: BTreeMap<String, ListedClient> = BTreeMap::new();
    // Each client's folder as last exchanged: the paths below are as last exchanged too, so a
    // folder renamed here and not pushed yet still takes its old name off them.
    let mut folders: BTreeMap<String, String> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.ext_id, c.name, coalesce(s.folder_rel, c.folder_rel, ''), s.rev
             FROM client c LEFT JOIN sync_segment s ON s.client_id = c.id
             WHERE c.deleted_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?;
        for row in rows {
            let (id, code, name, folder, rev) = row?;
            out.live.insert(id.clone());
            // Never exchanged: nothing of this client is in the bucket as far as this PC knows.
            let Some(rev) = rev else { continue };
            folders.insert(id.clone(), folder);
            by_id.insert(
                id.clone(),
                ListedClient {
                    id,
                    code,
                    name,
                    rev: rev.max(0) as u64,
                    docs: vec![],
                },
            );
        }
    }
    {
        // The path as last exchanged (what the bucket has); size and year as the catalog has
        // them now. A client's notes file is not a document (Q-115).
        let mut stmt = conn.prepare(
            "SELECT b.client_id, b.doc_id, b.rel_path, d.size, d.tax_year
             FROM sync_base b JOIN document d ON d.id = b.doc_id
             WHERE b.deleted = 0 AND b.client_id IS NOT NULL AND d.deleted_at IS NULL
               AND lower(b.rel_path) != '!fcinfo.ini' AND lower(b.rel_path) NOT LIKE '%/!fcinfo.ini'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<i32>>(4)?,
            ))
        })?;
        for row in rows {
            let (client, doc_id, rel_path, size, tax_year) = row?;
            let Some(c) = by_id.get_mut(&client) else {
                continue;
            };
            let folder = folders.get(&client).map_or("", String::as_str);
            c.docs.push(ListedDoc {
                doc_id,
                path: below(folder, &rel_path),
                size: size.max(0),
                tax_year,
            });
        }
    }
    out.clients = by_id.into_values().collect();
    for c in &mut out.clients {
        c.docs.sort_by(|a, b| {
            (a.path.as_str(), a.doc_id.as_str()).cmp(&(b.path.as_str(), b.doc_id.as_str()))
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_given_below_the_clients_folder() {
        assert_eq!(
            below("Acme (1001)", "Acme (1001)/Income Tax/2025/1040.pdf"),
            "Income Tax/2025/1040.pdf"
        );
        assert_eq!(
            below("Acme (1001)", r"acme (1001)\Permanent\EIN.pdf"),
            "Permanent/EIN.pdf"
        );
        // Not below it (a document that moved, not exchanged since): the path as it is.
        assert_eq!(below("Acme (1001)", "Other (2)/a.pdf"), "Other (2)/a.pdf");
        assert_eq!(
            below("Acme (1001)", "Acme (1001) old/a.pdf"),
            "Acme (1001) old/a.pdf"
        );
        assert_eq!(below("", "a.pdf"), "a.pdf");
    }
}
