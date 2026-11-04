//! PHASES P13 step 1: the performance pass on a 50,000-document synthetic catalog. Targets:
//! startup < 2 s, search < 100 ms, tree expand < 100 ms. Run in release:
//! `cargo test -p vf-app --release perf_50k -- --ignored --nocapture`.

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use vf_core::db::{Storage, params};
    use vf_fs::cabinet::CabinetFile;

    use crate::ui::browse::Browse;

    const CLIENTS: usize = 500;
    const PER: usize = 100;

    /// A made-up word for vocabulary index `k` (letters only, 4–9 long).
    fn pseudo_word(k: usize) -> String {
        const SYL: [&str; 16] = [
            "ka", "lo", "mi", "ne", "ru", "sa", "to", "vi", "be", "da", "fo", "gu", "ha", "je",
            "po", "ze",
        ];
        let mut w = String::new();
        let mut x = k + 17;
        for _ in 0..(2 + k % 3) {
            w.push_str(SYL[x % 16]);
            x /= 16;
            x += 3;
        }
        w
    }

    fn median(mut v: Vec<Duration>) -> Duration {
        v.sort();
        v[v.len() / 2]
    }

    #[test]
    #[ignore = "performance pass: run in release with --ignored"]
    fn perf_50k() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        let data = t.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        vf_fs::cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: uuid::Uuid::new_v4().to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: Some("Perf".into()),
            },
        )
        .unwrap();
        let catalog = data.join(crate::session::CATALOG_FILE);
        {
            let mut conn = vf_core::db::open(&catalog, Storage::Local).unwrap();
            vf_fs::doctypes::seed_if_missing(&root).unwrap();
            vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
            // The catalog knows the cabinet (as after its first open); rows below are synthetic
            // (no files: startup does not read files).
            vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
            let types: Vec<String> = {
                let mut st = conn.prepare("SELECT id FROM doc_type").unwrap();
                st.query_map([], |r| r.get(0))
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect()
            };
            let tx = conn.transaction().unwrap();
            let mut n: u64 = 7;
            for c in 0..CLIENTS {
                let cid = uuid::Uuid::now_v7().to_string();
                let folder = format!("Client {c:04} ({:04})", 1000 + c);
                tx.execute(
                    "INSERT INTO client (id, ext_id, name, folder_rel, extra, device_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, '{}', 'dev', 't')",
                    params![cid, format!("{:04}", 1000 + c), format!("Client {c:04}"), folder],
                )
                .unwrap();
                for d in 0..PER {
                    n = n
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let id = uuid::Uuid::now_v7().to_string();
                    let year = 2016 + (d % 10) as i64;
                    let hash = format!("{:064x}", n as u128 * 31 + d as u128);
                    tx.execute(
                        "INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash,
                           size, mime, filed_at, device_id, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1000, 'application/pdf', '2026-01-01T00:00:00Z', 'dev', 't')",
                        params![
                            id,
                            cid,
                            types[(n >> 40) as usize % types.len()],
                            year,
                            format!("{folder}/Income Tax/{year}/doc {d}.pdf"),
                            hash
                        ],
                    )
                    .unwrap();
                    // Page text like OCR of tax documents: a Zipf-distributed vocabulary of
                    // 3,000 words, form words in some documents, "tax" on every page.
                    let mut words: Vec<String> = Vec::with_capacity(200);
                    let mut m = n;
                    for _ in 0..200 {
                        m = m
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        let u = (m >> 11) as f64 / (1u64 << 53) as f64;
                        let k = (3000.0 * u * u * u) as usize;
                        words.push(pseudo_word(k));
                    }
                    words.push("tax".into());
                    let r = (n >> 24) % 100;
                    if r < 3 {
                        words.push("mortgage interest statement".into());
                    } else if r < 8 {
                        words.push("payroll quarterly report".into());
                    }
                    let mut text = words.join(" ");
                    if d == 42 {
                        text.push_str(&format!(" zephyrine{c}"));
                    }
                    tx.execute(
                        "INSERT INTO doc_fts (text, doc_id, page) VALUES (?1, ?2, 0)",
                        params![text, id],
                    )
                    .unwrap();
                }
            }
            tx.commit().unwrap();
            // An audit trail of a busy year: verified at every start.
            for _ in 0..50 {
                let mut tx = vf_core::tx::Transaction::new("perf");
                for _ in 0..1000 {
                    tx = tx.audit(vf_core::audit::AuditEntry::new(
                        "perf",
                        None,
                        serde_json::json!({}),
                    ));
                }
                tx.commit(
                    &mut conn,
                    &mut vf_core::tx::NoFileOps,
                    &vf_core::tx::Actor::device("dev"),
                )
                .unwrap();
            }
        }

        // Startup: open the catalog (recovery included), read the tree and grid, check the
        // audit chain, as the app does before its first frame.
        let mut starts = vec![];
        let mut browse = None;
        for _ in 0..3 {
            let t0 = Instant::now();
            let opened = crate::session::open(&data, "dev", &root).unwrap();
            let mut b = Browse::load(&opened.conn);
            b.set_root(&opened.root);
            let chain = vf_core::audit::verify(&opened.conn).unwrap();
            assert!(chain.broken.is_none());
            starts.push(t0.elapsed());
            browse = Some((b, opened));
        }
        let (mut b, opened) = browse.unwrap();
        assert_eq!(b.snap.docs.len(), CLIENTS * PER);

        // Tree expand: a client, then a year (the grid fills, sorted).
        let mut expands = vec![];
        for c in [0usize, 250, 499] {
            let t0 = Instant::now();
            b.set_scope(vf_core::browse::Scope::Client(c));
            b.set_scope(vf_core::browse::Scope::Folder {
                client: c,
                folder: "Income Tax/2020".into(),
            });
            expands.push(t0.elapsed());
            assert_eq!(b.visible.len(), PER / 10);
        }
        let t0 = Instant::now();
        b.set_scope(vf_core::browse::Scope::Cabinet);
        let whole = t0.elapsed();

        // Search: a common word, a rare one, and a common one within one client.
        let mut searches = vec![];
        let client = b.snap.clients[10].id.clone();
        for (text, client) in [
            ("mortgage interest", None),
            ("zephyrine250", None),
            ("payroll quarterly", Some(client.clone())),
        ] {
            let q = vf_index::search::Query {
                text: text.into(),
                client,
                ..Default::default()
            };
            let mut runs = vec![];
            for _ in 0..5 {
                let t0 = Instant::now();
                let hits = vf_index::search::search(&opened.conn, &q, 200).unwrap();
                runs.push(t0.elapsed());
                assert!(!hits.is_empty(), "{text}");
            }
            searches.push((text, median(runs)));
        }
        // Worst case, recorded not asserted: a word on every page of every document.
        let worst = {
            let mut runs = vec![];
            for client in [None, Some(client)] {
                let q = vf_index::search::Query {
                    text: "tax".into(),
                    client,
                    ..Default::default()
                };
                let t0 = Instant::now();
                let hits = vf_index::search::search(&opened.conn, &q, 200).unwrap();
                runs.push(t0.elapsed());
                assert!(!hits.is_empty());
            }
            runs
        };

        let start = median(starts);
        let expand = *expands.iter().max().unwrap();
        eprintln!("documents {}", CLIENTS * PER);
        eprintln!("startup (median of 3): {start:?}  target < 2 s");
        eprintln!("tree expand (worst of 3): {expand:?}  target < 100 ms");
        eprintln!("whole cabinet grid: {whole:?}");
        for (q, d) in &searches {
            eprintln!("search {q:?} (median of 5): {d:?}  target < 100 ms");
        }
        eprintln!(
            "worst case \"tax\" (on every page): whole cabinet {:?}, one client {:?}",
            worst[0], worst[1]
        );
        assert!(start < Duration::from_secs(2), "startup {start:?}");
        assert!(expand < Duration::from_millis(100), "expand {expand:?}");
        for (q, d) in &searches {
            assert!(*d < Duration::from_millis(100), "search {q}: {d:?}");
        }
    }
}
