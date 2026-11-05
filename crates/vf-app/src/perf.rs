//! PHASES P13 step 1: the performance pass on a 50,000-document synthetic catalog. Targets:
//! startup < 2 s, search < 100 ms, tree expand < 100 ms. Run in release:
//! `cargo test -p vf-app --release perf_50k -- --ignored --nocapture`. PA-34 adds `scale`
//! (~900K documents).

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
                    tx.execute(
                        "INSERT INTO doc_fts_doc (fts_row, doc_id) VALUES (last_insert_rowid(), ?1)",
                        params![id],
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

/// PA-34 step 9: the scale test for a cabinet of ~900K files in ~7,600 client folders. It writes
/// a real tree of small files (invented names, no client data) with a sentinel in each client
/// folder, catalogs it, adds page text, undo records and audit rows, and holds the owner's time
/// budgets (Q-68). Opt-in, in release:
///
/// `cargo test -p vf-app --release scale_900k -- --ignored --nocapture`
///
/// `VF_SCALE_DOCS` sets the number of documents (default 900,000) and `VF_SCALE_DIR` a folder
/// to build in (default a temporary folder; it needs a few GB). The folder is removed after.
#[cfg(test)]
mod scale {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use filer_core::clients::Client;
    use vf_core::db::{Storage, params};
    use vf_fs::cabinet::CabinetFile;

    use crate::ui::browse::Browse;

    const CLIENTS: usize = 7_600;

    fn median(mut v: Vec<Duration>) -> Duration {
        v.sort();
        v[v.len() / 2]
    }

    fn check(what: &str, took: Duration, budget: Duration, failed: &mut Vec<String>) {
        let ok = took < budget;
        eprintln!(
            "{:<42} {:>10.3} s   budget {:>6.1} s   {}",
            what,
            took.as_secs_f64(),
            budget.as_secs_f64(),
            if ok { "ok" } else { "OVER" }
        );
        if !ok {
            failed.push(format!("{what}: {took:?} (budget {budget:?})"));
        }
    }

    #[test]
    #[ignore = "scale test (PA-34): run in release with --ignored"]
    fn scale_900k() {
        let docs: usize = std::env::var("VF_SCALE_DOCS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(900_000);
        let clients = CLIENTS.min(docs.max(1));
        let per = (docs / clients).max(1);
        let total = clients * per;
        let tmp = match std::env::var("VF_SCALE_DIR") {
            Ok(d) => tempfile::tempdir_in(d).unwrap(),
            Err(_) => tempfile::tempdir().unwrap(),
        };
        let root = tmp.path().join("Clients");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let firm = uuid::Uuid::new_v4();
        vf_fs::cabinet::create(
            &root,
            &CabinetFile {
                cabinet_id: vf_core::new_id(),
                firm_id: firm.to_string(),
                schema_version: vf_core::db::SCHEMA_VERSION,
                layout_version: 1,
                created: vf_core::now_rfc3339(),
                name: Some("Scale".into()),
            },
        )
        .unwrap();

        // The tree: client folders with sentinels, a year folder per document's year.
        let t0 = Instant::now();
        for c in 0..clients {
            let folder = root.join(format!("Scale Client {c:05} (SC{c:05})"));
            std::fs::create_dir_all(&folder).unwrap();
            let cl = Client {
                id: format!("SC{c:05}"),
                uuid: uuid::Uuid::new_v4(),
                name: format!("Scale Client {c:05}"),
                extra: BTreeMap::new(),
            };
            filer_intake::sentinel::write_sentinel(&root, &folder, &cl, firm).unwrap();
            for y in 0..10.min(per) {
                std::fs::create_dir_all(folder.join(format!("Income Tax/{}", 2016 + y))).unwrap();
            }
            for d in 0..per {
                let year = 2016 + d % 10;
                std::fs::write(
                    folder.join(format!("Income Tax/{year}/doc {d}.pdf")),
                    format!("%PDF-1.4 scale {c} {d}"),
                )
                .unwrap();
            }
        }
        eprintln!(
            "built {total} files in {clients} client folders in {:.1} s",
            t0.elapsed().as_secs_f64()
        );

        let catalog = data.join(crate::session::CATALOG_FILE);
        let mut failed: Vec<String> = vec![];
        {
            let mut conn = vf_core::db::open(&catalog, Storage::Local).unwrap();
            vf_fs::doctypes::seed_if_missing(&root).unwrap();
            vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
            let t0 = Instant::now();
            let rep = vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
            eprintln!(
                "first index (every file read): {:.1} s",
                t0.elapsed().as_secs_f64()
            );
            assert_eq!(rep.docs_new, total, "{rep}");

            // Page text (as OCR leaves it), with a rare word in one document per 1,000.
            let t0 = Instant::now();
            let ids: Vec<String> = {
                let mut st = conn.prepare("SELECT id FROM document").unwrap();
                st.query_map([], |r| r.get(0))
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect()
            };
            let tx = conn.transaction().unwrap();
            {
                let mut ins = tx
                    .prepare("INSERT INTO doc_fts (text, doc_id, page) VALUES (?1, ?2, 0)")
                    .unwrap();
                let mut row = tx
                    .prepare(
                        "INSERT INTO doc_fts_doc (fts_row, doc_id) VALUES (last_insert_rowid(), ?1)",
                    )
                    .unwrap();
                for (i, id) in ids.iter().enumerate() {
                    let mut text = format!(
                        "Form W-2 wage and tax statement employer {} employee {} box {} federal income tax withheld",
                        i % 977,
                        i % 4099,
                        i % 20
                    );
                    if i % 1000 == 0 {
                        text.push_str(&format!(" quillfeather{}", i / 1000));
                    }
                    if i % 37 == 0 {
                        text.push_str(" mortgage interest statement");
                    }
                    ins.execute(params![text, id]).unwrap();
                    row.execute(params![id]).unwrap();
                }
            }
            tx.commit().unwrap();
            // Years of undo records and their audit rows (raw rows: the chain is not checked
            // here).
            let tx = conn.transaction().unwrap();
            for i in 0..30_000 {
                let tx_id = uuid::Uuid::now_v7().to_string();
                tx.execute(
                    "INSERT INTO undo_record (tx_id, user, device_id, ts, label, record)
                     VALUES (?1, ?2, 'dev', '2026-01-01T00:00:00Z', 'meta_edit', '{}')",
                    params![tx_id, format!("user{}", i % 12)],
                )
                .unwrap();
                for k in 0..3 {
                    tx.execute(
                        "INSERT INTO audit (ts, user, device_id, action, doc_id, detail, prev_hash, hash)
                         VALUES ('2026-01-01T00:00:00Z', ?1, 'dev', 'meta_edit', ?2, ?3, 'x', 'x')",
                        params![
                            format!("user{}", i % 12),
                            ids[(i * 3 + k) % ids.len()],
                            serde_json::json!({ "tx": tx_id }).to_string()
                        ],
                    )
                    .unwrap();
                }
            }
            tx.commit().unwrap();
            eprintln!(
                "page text, undo and audit rows: {:.1} s",
                t0.elapsed().as_secs_f64()
            );
        }

        // 1. Browse opens the cabinet (catalog open with recovery, the window read).
        let mut opens = vec![];
        let mut kept = None;
        for _ in 0..3 {
            let t0 = Instant::now();
            let opened = crate::session::open(&data, "dev", &root).unwrap();
            let mut b = Browse::load(&opened.conn);
            b.set_root(&opened.root);
            opens.push(t0.elapsed());
            kept = Some((b, opened));
        }
        let (mut b, opened) = kept.unwrap();
        assert_eq!(
            b.large,
            total > crate::ui::browse::LARGE,
            "read in windows above LARGE"
        );
        check(
            "Browse opens the cabinet (median of 3)",
            median(opens),
            Duration::from_secs(3),
            &mut failed,
        );

        // 2. A client's folder opens.
        let mut folders = vec![];
        for c in [0usize, clients / 2, clients - 1] {
            let t0 = Instant::now();
            b.set_scope(vf_core::browse::Scope::Client(c));
            b.keep_window(&opened.conn);
            b.set_scope(vf_core::browse::Scope::Folder {
                client: c,
                folder: "Income Tax/2020".into(),
            });
            b.keep_window(&opened.conn);
            folders.push(t0.elapsed());
        }
        check(
            "A client's folder opens (worst of 3)",
            *folders.iter().max().unwrap(),
            Duration::from_millis(500),
            &mut failed,
        );

        // 3. Search.
        let client = b.snap.clients[clients / 3].id.clone();
        // A word in one document of 1,000, from the middle of the catalog.
        let rare = format!("quillfeather{}", total / 2000);
        for (text, client) in [
            (rare.as_str(), None),
            ("mortgage interest", None),
            ("withheld", Some(client.clone())),
            ("doc 7", None),
        ] {
            let q = vf_index::search::Query {
                text: text.into(),
                client,
                ..Default::default()
            };
            let mut runs = vec![];
            for _ in 0..3 {
                let t0 = Instant::now();
                let hits = vf_index::search::search(&opened.conn, &q, 200).unwrap();
                runs.push(t0.elapsed());
                assert!(!hits.is_empty(), "{text}");
            }
            check(
                &format!("Search {text:?} (median of 3)"),
                median(runs),
                Duration::from_secs(2),
                &mut failed,
            );
        }

        // 4. The undo list.
        let t0 = Instant::now();
        let stack = vf_core::tx::undo_stack(
            &opened.conn,
            &vf_core::tx::Actor {
                user: Some("user3".into()),
                device_id: "dev".into(),
            },
            &[],
        )
        .unwrap();
        check(
            "The undo list",
            t0.elapsed(),
            Duration::from_millis(200),
            &mut failed,
        );
        assert!(!stack.is_empty());

        // 5. Planning a sync push: what changed (everything, never sent), and a round's
        //    entries.
        let keys = vf_sync::Keys::derive("scale", &[7u8; 32]);
        let t0 = Instant::now();
        let pending = vf_sync::catalog::pending(&opened.conn).unwrap();
        for c in pending.keys().take(100) {
            let folder = vf_sync::catalog::client_folder(&opened.conn, c)
                .unwrap()
                .unwrap_or_default();
            vf_sync::catalog::entries(&opened.conn, &keys, c, &folder).unwrap();
        }
        check(
            "Planning a sync push (a round)",
            t0.elapsed(),
            Duration::from_secs(30),
            &mut failed,
        );
        assert!(pending.len() >= clients);
        drop(b);
        drop(opened);

        // 6. Re-indexing an unchanged cabinet: size and time only, nothing read again.
        let mut conn = vf_core::db::open(&catalog, Storage::Local).unwrap();
        let t0 = Instant::now();
        let rep = vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
        check(
            "Re-index of an unchanged cabinet",
            t0.elapsed(),
            Duration::from_secs(120),
            &mut failed,
        );
        assert_eq!((rep.docs_new, rep.docs_changed), (0, 0), "{rep}");

        eprintln!("documents {total}");
        assert!(failed.is_empty(), "over budget: {failed:#?}");
    }
}
