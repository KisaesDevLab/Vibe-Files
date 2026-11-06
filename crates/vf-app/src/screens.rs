//! UI review helper (P13 QA): renders every tab and Admin page of the app on the generated
//! fixture cabinet to PNG files, for a person to look at. Not a regular test:
//! `cargo test -p vf-app screens -- --ignored`, with `VF_SCREENS_DIR` set to the output folder.
//!
//! `walkthrough` renders the frames of the landing-page walkthrough video (16:9, 1920x1080) on
//! the same generated fixture (invented names only): `VF_WALKTHROUGH_DIR` set to the output
//! folder; `scripts/walkthrough-video.ps1` turns them into the video.

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::HarnessBuilder;
    use egui_kittest::kittest::Queryable;

    use crate::app::VibeFilesApp;

    #[test]
    #[ignore = "writes screenshots for a UI review"]
    fn screens() {
        let Ok(out) = std::env::var("VF_SCREENS_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 7).unwrap();
        let data = t.path().join("data");
        // The fixture is a cabinet already: the first open indexes it.
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        // `VF_SCREENS_DARK=1`: the same screens in the dark theme.
        if std::env::var("VF_SCREENS_DARK").is_ok_and(|v| v == "1") {
            h.get_by_role_and_label(Role::Button, "Dark").click();
            h.run_steps(4);
        }
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        shot(&mut h, "00-wizard");
        h.get_by_role_and_label(Role::Button, "Open existing")
            .click();
        h.run_steps(8);
        h.get_by_label("Cabinet folder").click();
        h.step();
        h.get_by_label("Cabinet folder")
            .type_text(&root.display().to_string());
        h.step();
        h.get_by_role_and_label(Role::Button, "Open cabinet")
            .click();
        let start = std::time::Instant::now();
        while matches!(h.state().screen, crate::app::Screen::Busy { .. })
            || matches!(h.state().screen, crate::app::Screen::Wizard(_))
        {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
            assert!(start.elapsed().as_secs() < 30);
        }
        shot(&mut h, "01-browse-cabinet");
        {
            let crate::app::Screen::Open(o) = &mut h.state_mut().screen else {
                panic!()
            };
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
        }
        shot(&mut h, "02-browse-client");
        {
            let crate::app::Screen::Open(o) = &mut h.state_mut().screen else {
                panic!()
            };
            o.browse.set_scope(vf_core::browse::Scope::Folder {
                client: 0,
                folder: "Income Tax/2024".into(),
            });
            if !o.browse.visible.is_empty() {
                o.browse.selected.insert(o.browse.visible[0]);
            }
        }
        shot(&mut h, "03-browse-year-selected");
        for (i, tab) in ["Intake", "Search", "Viewer", "Sync", "Admin"]
            .iter()
            .enumerate()
        {
            h.get_by_role_and_label(Role::Button, tab).click();
            h.run_steps(8);
            shot(&mut h, &format!("{:02}-{}", 10 + i, tab.to_lowercase()));
        }
        for (i, page) in [
            "Folder import",
            "Print capture & parsers",
            "Audit trail",
            "Retention classes",
            "Disposition",
            "Legal holds",
            "Safeguards appendix",
            "Integrity sweep",
            "Trash",
        ]
        .iter()
        .enumerate()
        {
            if let Some(b) = h.query_by_role_and_label(Role::Button, page) {
                b.click();
                h.run_steps(8);
                shot(&mut h, &format!("{:02}-admin-{}", 20 + i, i));
            }
        }
        h.get_by_role_and_label(Role::Button, "Folder import")
            .click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "Scan the cabinet")
            .click();
        h.run_steps(8);
        shot(&mut h, "30-admin-import-scanned");
        // UltraTax's Document Names tab (PA-22).
        h.get_by_role_and_label(Role::Button, "Print capture & parsers")
            .click();
        h.run_steps(8);
        h.get_by_label("Document Names").click();
        h.run_steps(12);
        shot(&mut h, "31-admin-ultratax-documents");
        // Help (PA-25), on the ten-key article.
        h.key_press(egui::Key::F1);
        h.run_steps(4);
        h.state_mut().help.open_at("ten-key");
        h.run_steps(8);
        shot(&mut h, "40-help");
    }

    /// Steps until `ok`, up to 30 s; false if it never came.
    fn wait_for(
        h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
        ok: impl Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool,
    ) -> bool {
        let start = std::time::Instant::now();
        while !ok(h) {
            if start.elapsed().as_secs() > 30 {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.step();
        }
        true
    }

    fn open_mut<'a>(
        h: &'a mut egui_kittest::Harness<'static, VibeFilesApp>,
    ) -> &'a mut crate::app::OpenCabinet {
        let crate::app::Screen::Open(o) = &mut h.state_mut().screen else {
            panic!("no cabinet open")
        };
        o
    }

    /// Shows client `client`'s `folder` and selects its first document whose path has `like`.
    fn select_in(
        h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
        client: usize,
        folder: &str,
        like: &str,
    ) {
        let o = open_mut(h);
        o.browse.set_scope(vf_core::browse::Scope::Folder {
            client,
            folder: folder.into(),
        });
        o.browse.selected.clear();
        let hit = o
            .browse
            .visible
            .iter()
            .copied()
            .find(|&i| o.browse.snap.docs[i].rel_path.contains(like));
        if let Some(i) = hit.or(o.browse.visible.first().copied()) {
            o.browse.selected.insert(i);
        }
    }

    #[test]
    #[ignore = "writes the frames of the walkthrough video"]
    fn walkthrough() {
        let Ok(out) = std::env::var("VF_WALKTHROUGH_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 7).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        // Doc types from the file names, as in a firm that has been using it; one client is
        // left to tag.
        {
            let conn = vf_core::db::Connection::open(data.join("catalog.db")).unwrap();
            for (token, name) in [
                ("W-2", "W-2"),
                ("1099-INT", "1099-INT"),
                ("1099-DIV", "1099-DIV"),
                ("Organizer", "Organizer"),
                ("1040 Return", "Return copy — federal"),
                ("1065 Return", "Return copy — federal"),
                ("Engagement Letter", "Engagement letter"),
                ("Bank Statement", "Bank statement"),
                ("941", "Payroll tax return"),
                ("K-1", "K-1 (1065)"),
                ("Trial Balance", "Workpaper"),
            ] {
                conn.execute(
                    "UPDATE document SET doc_type_id = (SELECT id FROM doc_type WHERE name = ?2)
                     WHERE rel_path LIKE '%!_' || ?1 || '!_%' ESCAPE '!'
                       AND rel_path NOT LIKE 'Placeholt, Morgan (1002)%'",
                    (token, name),
                )
                .unwrap();
            }
        }
        // Three scans waiting in the office inbox.
        let inbox = t.path().join("Scans");
        std::fs::create_dir_all(&inbox).unwrap();
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();
        for (name, text) in [
            (
                "Demoreaux, Avery_1003_1099-B_2025.pdf",
                "1099-B 2025 SYNTHETIC TEST DOCUMENT - NOT REAL DATA",
            ),
            (
                "Placeholt, Morgan_1001_W-2_2025.pdf",
                "W-2 2025 SYNTHETIC TEST DOCUMENT - NOT REAL DATA",
            ),
            (
                "Synthetic Orchards LLC_1004_941_2025.pdf",
                "941 2025 SYNTHETIC TEST DOCUMENT - NOT REAL DATA",
            ),
        ] {
            filer_pdf::pdf::write_test_pdf(&inbox.join(name), &[text]);
        }

        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..6 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        h.get_by_role_and_label(Role::Button, "Open existing")
            .click();
        h.run_steps(8);
        h.get_by_label("Cabinet folder").click();
        h.step();
        h.get_by_label("Cabinet folder")
            .type_text(&root.display().to_string());
        h.step();
        h.get_by_role_and_label(Role::Button, "Open cabinet")
            .click();
        assert!(wait_for(&mut h, |h| matches!(
            h.state().screen,
            crate::app::Screen::Open(_)
        )));
        h.run_steps(8);

        // 1. The cabinet: every client folder, every document.
        if let Some(n) = h.query_by_label("Demoreaux, Avery  1003") {
            n.hover();
        }
        shot(&mut h, "01-cabinet");

        // 2. A client's year folder, a W-2 previewed.
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        wait_for(&mut h, |h| h.query_by_label("Open in Viewer").is_some());
        std::thread::sleep(std::time::Duration::from_millis(500));
        h.run_steps(10);
        if let Some(n) = h.query_by_label("Open in Viewer") {
            n.hover();
        }
        shot(&mut h, "02-client-preview");

        // 3. Its doc type, tax year and status.
        if let Some(n) = h.query_all_by_label("TAGS").last() {
            n.scroll_to_me();
        }
        h.run_steps(6);
        if let Some(n) = h.query_all_by_label("DOC TYPE").last() {
            n.hover();
        }
        shot(&mut h, "03-details");

        // 4. Intake: scans waiting, each with its client and folder.
        h.get_by_role_and_label(Role::Button, "Intake (3)").click();
        h.run_steps(10);
        if let Some(n) = h.query_by_label("Preview") {
            n.click();
        }
        wait_for(&mut h, |h| {
            h.query_all_by_label_contains("Placeholt, Morgan_1001_W-2")
                .next()
                .is_some()
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
        h.run_steps(20);
        if let Some(n) = h.query_all_by_label_contains("Commit").next() {
            n.hover();
        }
        shot(&mut h, "04-intake");

        // 5. Search, typed in two goes.
        h.get_by_role_and_label(Role::Button, "Search").click();
        h.run_steps(8);
        h.get_by_label("Search for").click();
        h.step();
        for (i, part) in ["1099", "-INT 2025"].iter().enumerate() {
            h.get_by_label("Search for").type_text(part);
            h.run_steps(4);
            shot(&mut h, &format!("05-search-{i}"));
        }
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(400));
            h.run_steps(10);
        }
        shot(&mut h, "05-search-2");

        // 6. The viewer, on a return.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(4);
        select_in(&mut h, 0, "Income Tax/2025", "1040");
        wait_for(&mut h, |h| h.query_by_label("Open in Viewer").is_some());
        h.get_by_role_and_label(Role::Button, "Open in Viewer")
            .scroll_to_me();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Open in Viewer")
            .click();
        assert!(
            wait_for(&mut h, |h| h.state().tab == crate::app::Tab::Viewer),
            "the viewer never opened"
        );
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(400));
            h.run_steps(10);
        }
        if let Some(n) = h.query_all_by_label("Stamps").next() {
            n.hover();
        }
        shot(&mut h, "06-viewer");

        // 7. Sync to the firm's own B2 bucket.
        h.get_by_role_and_label(Role::Button, "Sync").click();
        h.run_steps(8);
        if h.query_by_label("Bucket").is_some() {
            h.get_by_label("Bucket").click();
            h.step();
            h.get_by_label("Bucket").type_text("example-firm-cabinet");
        }
        h.run_steps(4);
        shot(&mut h, "07-sync");

        // 8. Admin: devices and Remote Desktop servers.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "Devices").click();
        h.run_steps(10);
        shot(&mut h, "08-servers");

        // 9. Compliance: retention classes.
        h.get_by_role_and_label(Role::Button, "Retention classes")
            .click();
        h.run_steps(10);
        shot(&mut h, "09-retention");

        // 10. Back to the cabinet, in the dark theme.
        h.get_by_role_and_label(Role::Button, "Dark").click();
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(4);
        open_mut(&mut h)
            .browse
            .set_scope(vf_core::browse::Scope::Client(2));
        open_mut(&mut h).browse.selected.clear();
        h.run_steps(10);
        shot(&mut h, "10-cabinet-dark");
    }
}
