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
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
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
        // A client's notes (PA-76): FileCenter's file in the client's folder, invented text.
        {
            let crate::app::Screen::Open(o) = &mut h.state_mut().screen else {
                panic!()
            };
            let folder = o.browse.snap.clients[0].folder_rel.clone().unwrap();
            std::fs::write(
                root.join(folder).join("!fcinfo.ini"),
                "[Fields]\r\n\r\n\r\n[Memo]\r\nPrefers a call before 10.\r\nOrganizer goes by mail, not the portal.\r\nExtension filed every year.\r\n\r\n[System]\r\n",
            )
            .unwrap();
            o.browse.selected.clear();
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
        }
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            h.run_steps(4);
        }
        shot(&mut h, "05-browse-client-notes");
        // The search boxes with something in them: each has its × (owner 2026-11-16).
        {
            let crate::app::Screen::Open(o) = &mut h.state_mut().screen else {
                panic!("open")
            };
            o.browse.client_filter = "farm".into();
            o.browse.name_filter = "return".into();
            o.browse.refresh();
        }
        h.run_steps(6);
        shot(&mut h, "06-browse-search-boxes");
        {
            let crate::app::Screen::Open(o) = &mut h.state_mut().screen else {
                panic!("open")
            };
            o.browse.client_filter.clear();
            o.browse.name_filter.clear();
            o.browse.refresh();
        }
        h.run_steps(4);
        for (i, tab) in ["Intake", "Viewer", "Sync", "Admin"].iter().enumerate() {
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
        // Admin > Bulk changes (PA-78): found by file name, all ticked.
        h.get_by_role_and_label(Role::Button, "Bulk changes")
            .click();
        h.run_steps(4);
        h.get_by_label("File name contains").click();
        h.step();
        h.get_by_label("File name contains").type_text("1099");
        h.step();
        h.get_by_role_and_label(Role::Button, "Find").click();
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            h.run_steps(4);
        }
        h.get_by_role_and_label(Role::Button, "Select all").click();
        h.run_steps(4);
        shot(&mut h, "32-admin-bulk-changes");
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

    /// Owner 2026-11-24 (Q24, Q35): Admin › Letter templates (the rich editor) and Admin ›
    /// Pricing plans (the packages grid), for a look: `VF_SCREENS_DIR` set to the output folder.
    #[test]
    #[ignore = "writes screenshots for a UI review"]
    fn letter_and_plan_screens() {
        let Ok(out) = std::env::var("VF_SCREENS_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        vf_letters::store::seed_if_missing(&root).unwrap();
        vf_portal::pricing::save(&root, &vf_portal::pricing::sample("1040", 2025)).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 1000.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let dark = std::env::var("VF_SCREENS_DARK").is_ok_and(|v| v == "1");
        if dark {
            h.get_by_role_and_label(Role::Button, "Dark").click();
            h.run_steps(4);
        }
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img
                    .save(out.join(format!("{}{name}.png", if dark { "dark-" } else { "" })))
                    .unwrap(),
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
        let start = std::time::Instant::now();
        while !matches!(h.state().screen, crate::app::Screen::Open(_)) {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
            assert!(start.elapsed().as_secs() < 30);
        }
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_all_by_label("Letter templates")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        h.query_all_by_label_contains("Engagement letter")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        shot(&mut h, "letters-1-editor");
        // In the first paragraph: B wraps a word; a field goes in at the cursor.
        h.get_all_by_label("Letter paragraph")
            .nth(1)
            .unwrap()
            .click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Table").click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Insert signature")
            .click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Spouse").click();
        h.run_steps(6);
        shot(&mut h, "letters-2-table-and-signature");
        // B with the cursor in a paragraph: typing goes on inside the bold marks.
        h.get_all_by_label("Letter paragraph")
            .nth(3)
            .unwrap()
            .click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "B").click();
        h.run_steps(4);
        h.event(egui::Event::Text("NOTE".into()));
        h.run_steps(4);
        let typed = {
            let o = match &h.state().screen {
                crate::app::Screen::Open(o) => o,
                _ => panic!("not open"),
            };
            serde_json::to_string(&o.admin.as_ref().unwrap().letter_templates.draft_blocks())
                .unwrap()
        };
        assert!(typed.contains("**NOTE**"), "{typed}");
        shot(&mut h, "letters-3-bold");
        h.get_all_by_label("Pricing plans").next().unwrap().click();
        h.run_steps(6);
        h.query_all_by_label_contains("2025 1040")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        shot(&mut h, "plans-1-packages");
        h.get_by_role_and_label(Role::Button, "Add service").click();
        h.run_steps(4);
        h.get_all_by_label("Add-ons").next().unwrap().click();
        h.run_steps(6);
        shot(&mut h, "plans-2-addons");
        // Q43: the first add-on dragged by its handle below the second.
        let names = |h: &egui_kittest::Harness<'static, VibeFilesApp>| {
            let o = match &h.state().screen {
                crate::app::Screen::Open(o) => o,
                _ => panic!("not open"),
            };
            o.admin.as_ref().unwrap().pricing_plans.addon_names()
        };
        let before = names(&h);
        let handles: Vec<egui::Rect> = h
            .get_all_by_label("Move add-on")
            .map(|n| n.rect())
            .collect();
        let (from, to) = (
            handles[0].center(),
            handles[1].center() + egui::vec2(0.0, 4.0),
        );
        h.event(egui::Event::PointerMoved(from));
        h.step();
        h.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        h.step();
        for k in 1..=6 {
            h.event(egui::Event::PointerMoved(
                from + (to - from) * (k as f32 / 6.0),
            ));
            h.step();
        }
        h.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        h.run_steps(3);
        let after = names(&h);
        assert_eq!(
            after,
            [before[1].clone(), before[0].clone()],
            "{before:?} then {after:?}"
        );
    }

    /// PA-103: the to-do lists window and Admin › To-do templates, against a portal stand-in on
    /// this PC answering with invented data. `VF_SCREENS_DIR` set to the output folder:
    /// `cargo test -p vf-app todo_screens -- --ignored`.
    #[test]
    #[ignore = "writes screenshots for a UI review"]
    fn todo_screens() {
        let Ok(out) = std::env::var("VF_SCREENS_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let url = fake_todo_portal();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 7).unwrap();
        // PA-104: an invented 2025 return in each client folder, cataloged when the cabinet opens.
        crate::ui::test_util::pdfium_first();
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') {
                let dir = e.path().join("Income Tax").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join("2025 1040 Tax Return.pdf"),
                    crate::ui::test_util::SAMPLE_RETURN,
                );
            }
        }
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &format!("vpk_{}", "x".repeat(32)), 0)
            .unwrap()
            .with_todo_lists(true);
        vf_portal::settings::save(&root, &s).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
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
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
        }
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "To-do lists…")
            .click();
        assert!(wait_for(&mut h, |h| h
            .query_by_label("2026 tax documents")
            .is_some()));
        shot(&mut h, "vf-1-todo-lists");
        h.query_all_by_role_and_label(Role::Button, "Open")
            .next()
            .unwrap()
            .click();
        assert!(wait_for(&mut h, |h| h
            .query_by_label("1. W-2 from each employer")
            .is_some()));
        shot(&mut h, "vf-2-todo-list-open");
        h.get_by_role_and_label(Role::Button, "Edit").scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Edit").click();
        h.run_steps(6);
        shot(&mut h, "vf-3-todo-edit");
        h.get_by_role_and_label(Role::Button, "Cancel")
            .scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Cancel").click();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Hide").scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Hide").click();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "+ New list")
            .scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "+ New list").click();
        h.run_steps(6);
        shot(&mut h, "vf-4-todo-new-list");
        // PA-104: a new list from last year's return.
        h.get_by_role_and_label(Role::Button, "Cancel")
            .scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Cancel").click();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "From last year's return…")
            .click();
        h.run_steps(6);
        shot(&mut h, "vf-7-from-return");
        {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::RadioButton)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l.ends_with("2025 1040 Tax Return.pdf"))
                })
                .expect("the invented return is offered")
                .click();
        }
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Read it").click();
        assert!(wait_for(&mut h, |h| h
            .query_by_label("Items read from last year's return")
            .is_some()));
        h.run_steps(4);
        shot(&mut h, "vf-8-return-review");
        let use_btn = h.get_by_role_and_label(Role::Button, "Use these 8 items");
        use_btn.scroll_to_me();
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Use these 8 items")
            .click();
        h.run_steps(6);
        shot(&mut h, "vf-9-return-new-list");
        open_mut(&mut h).browse.portal.todo = None;
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(8);
        shot(&mut h, "vf-5-admin-portal-switch");
        h.get_by_role_and_label(Role::Button, "To-do templates")
            .click();
        assert!(wait_for(&mut h, |h| h
            .query_by_label("1040 year-end documents")
            .is_some()));
        // The grid settles its column widths over a few frames.
        h.run_steps(6);
        h.query_all_by_role_and_label(Role::Button, "Send to clients…")
            .next()
            .unwrap()
            .click();
        h.run_steps(8);
        shot(&mut h, "vf-6-admin-todo-templates");
    }

    /// Addendum §12 slice 2 (tax return delivery) walkthrough against a real Vibe Portal: the
    /// Deliver window sends the return and the 8879 link, the client (headless Edge) opens the
    /// return and says they signed, staff confirm, and both step PDFs with their log pages are
    /// fetched and committed by Intake into `Income Tax\2025\Delivery`. Set `VF_WALK_URL`,
    /// `VF_WALK_KEY`, `VF_WALK_LOG`, `VF_WALK_CLIENT` (`scripts/delivery-walkthrough/client.mjs`)
    /// and `VF_WALK_DIR`; the portal must have deliveries on.
    #[test]
    #[ignore = "needs a running portal"]
    fn delivery_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') {
                let dir = e.path().join("Income Tax").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join("2025 Form 1040 Return - Sample.pdf"),
                    crate::ui::test_util::SAMPLE_RETURN,
                );
            }
        }
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        // What Admin › Portal's Save does when Tax return delivery is ticked.
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let folder = c.folder_rel.clone().unwrap();
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        // Jordan is invited; the window shares the return (held for the invitation, P98).
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Owner",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}) on the portal; Jordan invited; the return in the client folder",
            c.name, c.ext_id
        ));

        // ---- Step 1: Deliver… sends the return and the 8879 link.
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "2025 Form 1040 Return - Sample.pdf")
        });
        shot(&mut h, "00-deliver-read");
        h.get_by_label("eSignature link address").click();
        h.step();
        h.get_by_label("eSignature link address")
            .type_text("https://esign.example-tax.test/s/walkthrough");
        h.run_steps(3);
        shot(&mut h, "01-deliver-window");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        shot(&mut h, "02-deliver-sent");
        let sent = p.deliveries(Some(&c.id)).unwrap();
        assert_eq!(sent.len(), 1);
        let did = sent[0].id.clone();
        note(format!(
            "Step 1: sent {} with the return and the Form 8879 link; the portal says {}",
            sent[0].title,
            sent[0].line()
        ));

        // ---- Step 2: the client opens the return and says they signed Form 8879.
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                "jordan.sample@example.com",
                "deliver",
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client's steps (see client-deliver.json)");
        let now = p.delivery(&did).unwrap();
        assert_eq!(
            now.step(vf_portal::delivery::RETURN)
                .map(|s| s.state.as_str()),
            Some("done")
        );
        assert_eq!(
            now.step(vf_portal::delivery::ESIGN)
                .map(|s| s.state.as_str()),
            Some("waiting")
        );
        note("Step 2: the client opened the return (step done) and said they signed Form 8879 (waiting)".into());

        // ---- Step 3: staff confirm in the window.
        click_read_again(&mut h);
        wait_long(&mut h, "the window says signed", &|h| {
            said(h, "The client says they signed it.")
        });
        shot(&mut h, "03-deliver-waiting");
        h.get_by_role_and_label(Role::Button, "Confirm signed")
            .click();
        wait_long(&mut h, "confirmed", &|h| said(h, "Form 8879 confirmed."));
        assert_eq!(p.delivery(&did).unwrap().status, "complete");
        note("Step 3: Confirm signed: the delivery is complete".into());
        {
            let o = open_mut(&mut h);
            o.browse.portal.delivery = None;
        }

        // ---- Step 4: both step PDFs reach the Portal inbox and Intake files them.
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let names = |inbox: &std::path::Path| -> Vec<String> {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(" Delivery "))
                .collect()
        };
        wait_long(&mut h, "both step PDFs fetched", &|_| {
            names(&inbox).len() == 2
        });
        note(format!("Step 4: in the Portal inbox: {:?}", names(&inbox)));
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        wait_long(&mut h, "both rows ready", &|h| said(h, "Commit 2 files"));
        shot(&mut h, "04-intake-step-pdfs");
        h.query_all_by_role_and_label(Role::Button, "Commit 2 files")
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        let dir = root
            .join(&folder)
            .join("Income Tax")
            .join("2025")
            .join("Delivery");
        for want in [
            "2025 Delivery 2 - Tax return.pdf",
            "2025 Delivery 5 - Form 8879.pdf",
        ] {
            let path = dir.join(want);
            wait_long(&mut h, want, &|_| path.exists());
        }
        let start = std::time::Instant::now();
        let types: Vec<String> = loop {
            let got: Vec<String> = {
                let o = open_mut(&mut h);
                let mut st = o
                    .opened
                    .conn
                    .prepare(
                        "SELECT t.name FROM document d JOIN doc_type t ON t.id = d.doc_type_id
                         WHERE d.rel_path LIKE '%Income Tax/2025/Delivery/%' AND d.deleted_at IS NULL
                         ORDER BY t.name",
                    )
                    .unwrap();
                st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
            };
            if got.len() == 2 || start.elapsed().as_secs() > 60 {
                break got;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        };
        assert_eq!(
            types,
            ["Delivery — 8879 record", "Delivery — return record"]
        );
        note(format!(
            "Step 4: committed into {folder}\\Income Tax\\2025\\Delivery with doc types {types:?}"
        ));
        // The copy in the client folder.
        assert!(!vf_portal::delivery::read_copies(&root, &folder).is_empty());
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Owner 2026-11-27: the frames of the staff training video and guide "Delivering a tax
    /// return with Vibe Files", from the real app against a real portal (made-up clients only).
    /// 1920×1080 frames, `tNN-*.png` in `VF_WALK_DIR`, what to look at outlined. Same variables
    /// as `delivery_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn training_deliver_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') {
                let dir = e.path().join("Income Tax").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join("2025 Form 1040 Return - Sample.pdf"),
                    crate::ui::test_util::SAMPLE_RETURN,
                );
            }
        }
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let mut s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true)
            .with_balances(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        // A frame, with what to look at outlined in amber.
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
                }
            };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        // A person, not a business: a 1040.
        let (ci, c) = {
            let o = open_mut(&mut h);
            let (i, c) = o
                .browse
                .snap
                .clients
                .iter()
                .enumerate()
                .find(|(_, c)| c.name.contains(", "))
                .map(|(i, c)| (i, c.clone()))
                .unwrap();
            (i, c)
        };
        let first = c.name.split(", ").nth(1).unwrap_or("Jordan").to_string();
        let last = c.name.split(", ").next().unwrap_or("Sample").to_string();
        let email = format!(
            "{}.{}@example.com",
            first.to_lowercase(),
            last.to_lowercase()
        );
        vf_fs::contacts::set_by_hand(
            &root,
            &c.id,
            &vf_fs::contacts::Row {
                first: first.clone(),
                last: last.clone(),
                email: email.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some(&email),
            None,
            &format!("{first} {last}"),
            "Taxpayer",
        )
        .unwrap();
        // Owner 2026-11-27: more cards for the bulk actions: the other clients' Form 8879 to
        // sign at the office or by mail (the business has nobody on the portal yet).
        let rest: Vec<(String, String, String)> = {
            let o = open_mut(&mut h);
            o.browse
                .snap
                .clients
                .iter()
                .filter(|x| x.id != c.id)
                .take(3)
                .map(|x| (x.id.clone(), x.ext_id.clone(), x.name.clone()))
                .collect()
        };
        for (k, (id, ext, name)) in rest.iter().enumerate() {
            p.put_client(id, ext, name, None).unwrap();
            let person = name.contains(", ");
            if person {
                let f = name.split(", ").nth(1).unwrap_or("Pat");
                let l = name.split(", ").next().unwrap_or("Sample");
                p.invite(
                    id,
                    Some(&format!(
                        "{}.{}@example.com",
                        f.to_lowercase(),
                        l.to_lowercase()
                    )),
                    None,
                    &format!("{f} {l}"),
                    "Taxpayer",
                )
                .unwrap();
            }
            p.create_delivery(&vf_portal::delivery::NewDelivery {
                client_id: id.clone(),
                tax_year: 2025,
                return_type: if person { "1040" } else { "1065" }.into(),
                title: "Your 2025 tax return".into(),
                steps: vf_portal::delivery::StepsIn {
                    esign8879: true,
                    ..Default::default()
                },
                locks: Default::default(),
                assigned_staff_id: None,
                notify: false,
                hold: false,
                remind_days: 7,
                kind: None,
                covers: Vec::new(),
                return_year: None,
                esign_mode: Some(if k == 1 { "mail" } else { "office" }.into()),
            })
            .unwrap();
            // The people accept their invitations (off camera), so Remind ticked reaches them;
            // the business keeps nobody on the portal.
            if person {
                let f = name.split(", ").nth(1).unwrap_or("Pat");
                let l = name.split(", ").next().unwrap_or("Sample");
                let ok = std::process::Command::new("node")
                    .args([
                        &client_js,
                        &url,
                        &out.display().to_string(),
                        &log,
                        &format!("{}.{}@example.com", f.to_lowercase(), l.to_lowercase()),
                        "accept",
                    ])
                    .status()
                    .is_ok_and(|s| s.success());
                assert!(ok, "{name} accepts the invitation");
            }
        }
        // The firm's Assign new cards to: Pat Preparer.
        let pat = p
            .delivery_staff(&c.id)
            .unwrap()
            .into_iter()
            .find(|x| x.name == "Pat Preparer")
            .unwrap();
        s = s.with_default_staff(Some(vf_portal::settings::DefaultStaff {
            id: pat.id.clone(),
            name: pat.name.clone(),
        }));
        vf_portal::settings::save(&root, &s).unwrap();
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(ci));
            o.browse.portal.reread();
        }
        h.run_steps(10);
        wait_long(&mut h, "the panel", &|h| {
            h.query_by_role_and_label(Role::Button, "Deliver…")
                .is_some()
        });

        // ---- 2. Browse: the client, Deliver….
        shot(&mut h, "t02a-browse-client", &[]);
        let r = button(&h, "Deliver…");
        shot(&mut h, "t02b-deliver-button", &[r]);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "2025 Form 1040 Return - Sample.pdf")
        });
        h.run_steps(10);

        // ---- 3. Kind, year, return type, title.
        let marks = [h.get_by_label("Card title").rect()];
        shot(&mut h, "t03-deliver-top", &marks);

        // ---- 4. Letters: the kit letter's fill-ins, then the firm's own letter; preview.
        h.get_by_label("Letters to sign").click();
        h.run_steps(4);
        h.query_all_by_label_contains("individual income tax return (Form 1040)")
            .next()
            .expect("the kit's 1040 letter")
            .click();
        h.run_steps(4);
        wait_long(&mut h, "the boxes", &|h| said(h, "Fill in for this client"));
        h.get_by_label("[State]").click();
        h.step();
        h.get_by_label("[State]").type_text("Kansas");
        h.get_by_label("[Frequency]").click();
        h.step();
        h.get_by_label("[Frequency]").type_text("quarterly");
        h.run_steps(4);
        let r = h.get_by_label("[State]").rect();
        shot(&mut h, "t04a-fill-ins", &[r]);
        h.query_all_by_label_contains("individual income tax return (Form 1040)")
            .next()
            .unwrap()
            .click();
        h.run_steps(4);
        let r = button(&h, "Preview letters…");
        shot(&mut h, "t04b-letters-preview", &[r]);

        // ---- 5. The return PDF.
        let r = h.get_by_label("Tax return PDF").rect();
        h.get_by_label("Tax return PDF").scroll_to_me();
        h.run_steps(4);
        let _ = r;
        let r = h.get_by_label("Tax return PDF").rect();
        shot(&mut h, "t05-return-pdf", &[r]);

        // ---- 6. Form 8879: the eSignature link.
        h.get_by_label("eSignature link address").scroll_to_me();
        h.run_steps(4);
        h.get_by_label("eSignature link address").click();
        h.step();
        h.get_by_label("eSignature link address")
            .type_text("https://esign.example-tax.test/s/training");
        h.run_steps(4);
        let r = h.get_by_label("eSignature link address").rect();
        shot(&mut h, "t06-esign-link", &[r]);

        // ---- 7. Package and fee: shown, then left off.
        h.get_by_label("Package and fee").scroll_to_me();
        h.run_steps(3);
        h.get_by_label("Package and fee").click();
        h.run_steps(4);
        h.get_by_label("Return fee").click();
        h.step();
        h.get_by_label("Return fee").type_text("450");
        h.run_steps(4);
        let r = h.get_by_label("Return fee").rect();
        shot(&mut h, "t07-fee", &[r]);
        h.get_by_label("Package and fee").click();
        h.run_steps(4);

        // ---- 8. Assigned to, reminders, Send to client.
        h.get_by_role_and_label(Role::Button, "Send to client")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Send to client");
        shot(&mut h, "t08a-send", &[r]);
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        let r = h.get_by_label("Sent to the client.").rect();
        shot(&mut h, "t08b-sent", &[r]);
        let did = p.deliveries(Some(&c.id)).unwrap()[0].id.clone();

        // ---- 12 (shown later). The letters signed on paper (the letters' button is the upper
        // one; Form 8879 has its own).
        h.get_by_label("Letters to sign").scroll_to_me();
        h.run_steps(4);
        h.query_all_by_role_and_label(Role::Button, "Signed on paper…")
            .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .unwrap()
            .click();
        h.run_steps(4);
        h.get_by_label("Signed on paper note").click();
        h.step();
        h.get_by_label("Signed on paper note")
            .type_text("Signed at the office");
        h.run_steps(3);
        let r = button(&h, "Mark signed");
        shot(&mut h, "t12a-paper", &[r]);
        h.get_by_role_and_label(Role::Button, "Mark signed").click();
        wait_long(&mut h, "marked", &|h| said(h, "marked signed on paper"));
        assert!(
            said(&h, "The letters marked signed on paper"),
            "the letters, not Form 8879"
        );
        shot(&mut h, "t12b-paper-done", &[]);
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(6);

        // ---- 9. The client's details line.
        open_mut(&mut h).browse.portal.look_at_deliveries_now();
        wait_long(&mut h, "the panel line", &|h| {
            said(h, "Tax return delivery")
        });
        let r = h
            .query_all_by_label_contains("Tax return delivery")
            .next()
            .unwrap()
            .rect();
        shot(&mut h, "t09-panel-line", &[r]);

        // ---- The client opens the return and says they signed Form 8879 (off camera).
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                &email,
                "deliver",
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client's steps");

        // ---- 10. Admin › Portal › Deliveries.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Deliveries").click();
        wait_long(&mut h, "the list", &|h| said(h, "4 listed"));
        h.run_steps(8);
        shot(&mut h, "t10a-deliveries", &[]);
        let r = h.get_by_label("Filter").rect();
        h.get_by_label("Filter").click();
        h.step();
        h.get_by_label("Filter").type_text(&first);
        h.run_steps(4);
        shot(&mut h, "t10b-filter", &[r]);
        h.get_by_label("Tick row").click();
        h.run_steps(4);
        let r = button(&h, "Remind ticked");
        shot(&mut h, "t10c-remind", &[r]);
        h.get_by_label("Tick row").click();
        h.run_steps(3);

        // ---- 11. Confirm signed.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "signed", &|h| {
            said(h, "The client says they signed it.")
        });
        h.get_by_role_and_label(Role::Button, "Confirm signed")
            .scroll_to_me();
        h.run_steps(4);
        let marks = [
            h.get_by_label("The client says they signed it.").rect(),
            button(&h, "Confirm signed"),
        ];
        shot(&mut h, "t11a-client-signed", &marks);
        h.get_by_role_and_label(Role::Button, "Confirm signed")
            .click();
        wait_long(&mut h, "confirmed", &|h| said(h, "Form 8879 confirmed."));
        shot(&mut h, "t11b-confirmed", &[]);
        assert_eq!(p.delivery(&did).unwrap().status, "complete");
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(4);

        use egui_kittest::kittest::NodeT;
        // ---- 12b. Many at once: Admin › Portal › Deliveries, Form 8879, Tick all shown,
        // Remind ticked, then Mark ticked signed on paper.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Deliveries").click();
        h.run_steps(6);
        if h.query_by_label("Clear").is_some() {
            h.get_by_label("Clear").click();
            h.run_steps(3);
        }
        click_read_again(&mut h);
        wait_long(&mut h, "the other cards", &|h| said(h, "3 listed"));
        h.get_by_label("Form 8879").click();
        h.run_steps(6);
        let r = button(&h, "Tick all shown");
        h.get_by_role_and_label(Role::Button, "Tick all shown")
            .click();
        h.run_steps(4);
        shot(&mut h, "t14a-tick-all", &[r]);
        h.get_by_role_and_label(Role::Button, "Remind ticked")
            .click();
        h.run_steps(4);
        let r = button(&h, "Remind");
        shot(&mut h, "t14b-remind-ask", &[r]);
        h.get_by_role_and_label(Role::Button, "Remind").click();
        wait_long(&mut h, "reminded", &|h| {
            said(h, "eminded") && !said(h, "Reminding")
        });
        h.run_steps(6);
        let r = h
            .query_all_by_label_contains("eminded")
            .last()
            .unwrap()
            .rect();
        shot(&mut h, "t14c-reminded", &[r]);
        if h.query_all_by_role(Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l == "Mark ticked signed on paper… (0)")
            })
            .is_some()
        {
            h.get_by_role_and_label(Role::Button, "Tick all shown")
                .click();
            h.run_steps(4);
        }
        h.query_all_by_role(Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Mark ticked signed on paper"))
            })
            .expect("the Mark ticked button")
            .click();
        h.run_steps(4);
        h.get_by_label("Signed on paper note").click();
        h.step();
        h.get_by_label("Signed on paper note")
            .type_text("Signed at the office");
        h.run_steps(3);
        let r = h
            .query_all_by_role(Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Mark ") && l.ends_with(" signed"))
            })
            .expect("the Mark n signed button")
            .rect();
        shot(&mut h, "t15a-mark-ask", &[r]);
        h.query_all_by_role(Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Mark ") && l.ends_with(" signed"))
            })
            .unwrap()
            .click();
        wait_long(&mut h, "marked", &|h| {
            said(h, "marked signed on paper. ") || said(h, " marked signed on paper.")
        });
        h.run_steps(8);
        shot(&mut h, "t15b-marked", &[]);

        // ---- 13. The step PDFs filed by Intake.
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let count = |inbox: &std::path::Path| {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains(" Delivery "))
                .count()
        };
        let start = std::time::Instant::now();
        let mut asked = std::time::Instant::now();
        // Casey's three, and the other clients' Form 8879 signed on paper in bulk.
        while count(&inbox) < 6 && start.elapsed().as_secs() < 120 {
            if asked.elapsed().as_secs() >= 10 {
                open_mut(&mut h).browse.portal.fetch_now = true;
                asked = std::time::Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.step();
        }
        let got: Vec<String> = std::fs::read_dir(&inbox)
            .map(|r| {
                r.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        eprintln!("Portal inbox: {got:?}");
        let n = count(&inbox);
        assert!(n >= 6, "the step PDFs fetched");
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        let commit = format!("Commit {n} files");
        wait_long(&mut h, "rows ready", &|h| said(h, &commit));
        let r = h
            .query_all_by_role_and_label(Role::Button, &commit)
            .last()
            .unwrap()
            .rect();
        shot(&mut h, "t13a-intake", &[r]);
        h.query_all_by_role_and_label(Role::Button, &commit)
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(10);
        let dir = root
            .join(c.folder_rel.as_ref().unwrap())
            .join("Income Tax")
            .join("2025")
            .join("Delivery");
        let path = dir.join("2025 Delivery 5 - Form 8879.pdf");
        wait_long(&mut h, "filed", &|_| path.exists());
        // In the catalog too (Intake records them as it finishes, on its own tab).
        let start = std::time::Instant::now();
        loop {
            let n: i64 = {
                let o = open_mut(&mut h);
                o.opened
                    .conn
                    .query_row(
                        "SELECT count(*) FROM document WHERE rel_path LIKE '%Income Tax/2025/Delivery/%' AND deleted_at IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0)
            };
            if n >= 6 {
                break;
            }
            assert!(
                start.elapsed().as_secs() < 120,
                "the catalog has the filed PDFs: {n}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        h.state_mut().tab = crate::app::Tab::Browse;
        h.run_steps(10);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(ci));
        }
        h.run_steps(6);
        // Intake files the six on its thread; Refresh until this client's three are listed.
        let start = std::time::Instant::now();
        while !(said(&h, "2025 Delivery 1 - Letters.pdf")
            && said(&h, "2025 Delivery 5 - Form 8879.pdf"))
        {
            assert!(
                start.elapsed().as_secs() < 180,
                "the filed PDFs listed: never"
            );
            h.get_by_role_and_label(Role::Button, "Refresh").click();
            for _ in 0..50 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                h.step();
            }
        }
        h.run_steps(6);
        let filed: Vec<egui::Rect> = [
            "2025 Delivery 1 - Letters.pdf",
            "2025 Delivery 2 - Tax return.pdf",
            "2025 Delivery 5 - Form 8879.pdf",
        ]
        .iter()
        .filter_map(|n| h.query_all_by_label_contains(n).next().map(|x| x.rect()))
        .collect();
        let r = filed.iter().copied().reduce(|a, b| a.union(b)).unwrap();
        // The whole rows, not only their tick boxes.
        let r = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 740.0, r.max.y));
        shot(&mut h, "t13b-filed", &[r]);
    }
    /// Owner 2026-11-27: the frames of four training videos against a real portal: sharing
    /// documents through the portal, a secure link to someone outside the client, Balance
    /// owed…, and End engagement…. Same variables as `training_agreements_walkthrough`; run with
    /// `VF_TRAIN=1 VF_MOBILE=1` for the client's phone screens (train-*.png).
    #[test]
    #[ignore = "needs a running portal"]
    fn training_portal_more_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true)
            .with_balances(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        for (_, name) in vf_portal::delivery::AGREEMENT_STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::AGREEMENT_FOLDER, true)
                .unwrap();
        }
        vf_fs::doctypes::ensure(
            &root,
            vf_portal::inbox::RECEIPT_DOC_TYPE,
            "Billing/{Year}",
            true,
        )
        .unwrap();
        vf_ingest::parsers::sync_filer_rules_report(&root).unwrap();
        vf_letters::store::seed_if_missing(&root).unwrap();
        // The firm has filled in its Terms and Conditions once (A17).
        let mut terms = vf_letters::store::load_all(&root)
            .unwrap()
            .into_iter()
            .find(|t| t.kind == vf_letters::TERMS)
            .unwrap();
        let mut json = serde_json::to_string(&terms.blocks).unwrap();
        for (k, v) in [
            ("[X]", "30"),
            ("[Portal Name]", "Sample Portal"),
            ("[State Name]", "Ohio"),
            (
                "[Name of State or Federal Court Desired]",
                "Franklin County, Ohio",
            ),
        ] {
            json = json.replace(k, v);
        }
        terms.blocks = serde_json::from_str(&json).unwrap();
        terms.sample = false;
        vf_letters::store::save(&root, &terms).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        // A frame, with what to look at outlined in amber.
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 120, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
                }
            };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };
        let text_rect = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.query_all_by_label_contains(l)
                .next()
                .unwrap_or_else(|| panic!("{l}: not shown"))
                .rect()
        };
        let _type_into =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, v: &str| {
                h.get_by_label(label).click();
                h.step();
                h.key_press(egui::Key::End);
                for _ in 0..12 {
                    h.key_press(egui::Key::Backspace);
                    h.step();
                }
                h.get_by_label(label).type_text(v);
                h.run_steps(3);
            };
        let admin_page = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, page: &str| {
            h.get_by_role_and_label(Role::Button, "Admin").click();
            h.run_steps(6);
            h.get_by_role_and_label(Role::Button, "Portal").click();
            h.run_steps(6);
            h.get_all_by_label(page).next().unwrap().scroll_to_me();
            h.run_steps(4);
            h.get_all_by_label(page).next().unwrap().click();
            h.run_steps(8);
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        // A person (a 1040, priced from the plan) and a business (the agreement).
        let clients = open_mut(&mut h).browse.snap.clients.clone();
        let (pi, person) = clients
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.contains(", "))
            .map(|(i, c)| (i, c.clone()))
            .unwrap();
        let (bi, biz) = clients
            .iter()
            .enumerate()
            .find(|(i, _)| *i != pi)
            .map(|(i, c)| (i, c.clone()))
            .unwrap();
        let folder = biz.folder_rel.clone().unwrap();
        let first = person
            .name
            .split(", ")
            .nth(1)
            .unwrap_or("Jordan")
            .to_string();
        let last = person
            .name
            .split(", ")
            .next()
            .unwrap_or("Sample")
            .to_string();
        let p_email = format!(
            "{}.{}@example.com",
            first.to_lowercase(),
            last.to_lowercase()
        );
        vf_fs::contacts::set_by_hand(
            &root,
            &person.id,
            &vf_fs::contacts::Row {
                first: first.clone(),
                last: last.clone(),
                email: p_email.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        vf_fs::clients::set_entity_type(&root, &biz.id, Some("llc")).unwrap();
        for (k, v) in [
            ("first", "Pat"),
            ("last", "Sample"),
            ("email", "pat.sample@example.com"),
        ] {
            vf_fs::contacts::set_one_by_hand(&root, &biz.id, k, v).unwrap();
        }
        let p = crate::portal::client(&s);
        p.put_client(&person.id, &person.ext_id, &person.name, None)
            .unwrap();
        p.invite(
            &person.id,
            Some(&p_email),
            None,
            &format!("{first} {last}"),
            "Taxpayer",
        )
        .unwrap();
        p.put_client(&biz.id, &biz.ext_id, &biz.name, None).unwrap();
        p.invite(
            &biz.id,
            Some("pat.sample@example.com"),
            None,
            "Pat Sample",
            "Owner",
        )
        .unwrap();

        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
            }
        };
        vf_letters::partners::save(
            &root,
            &[vf_letters::partners::Partner {
                name: "Morgan Example".into(),
                title: "Partner".into(),
                code: String::new(),
            }],
        )
        .unwrap();
        // A third client for Balance owed.
        let (mi, third) = clients
            .iter()
            .enumerate()
            .find(|(i, c)| *i != pi && *i != bi && c.name.contains(", "))
            .map(|(i, c)| (i, c.clone()))
            .unwrap();
        let m_first = third.name.split(", ").nth(1).unwrap().to_string();
        let m_last = third.name.split(", ").next().unwrap().to_string();
        let m_email = format!(
            "{}.{}@example.com",
            m_first.to_lowercase(),
            m_last.to_lowercase()
        );
        vf_fs::contacts::set_by_hand(
            &root,
            &third.id,
            &vf_fs::contacts::Row {
                first: m_first.clone(),
                last: m_last.clone(),
                email: m_email.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        p.put_client(&third.id, &third.ext_id, &third.name, None)
            .unwrap();
        p.invite(
            &third.id,
            Some(&m_email),
            None,
            &format!("{m_first} {m_last}"),
            "Taxpayer",
        )
        .unwrap();
        let node = |args: &[&str]| {
            let mut all = vec![
                client_js.as_str(),
                url.as_str(),
                out.to_str().unwrap(),
                log.as_str(),
            ];
            all.extend_from_slice(args);
            let ok = std::process::Command::new("node")
                .args(&all)
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "the client's side: {args:?}");
        };
        let pick = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, i: usize| {
            h.get_by_role_and_label(Role::Button, "Browse").click();
            h.run_steps(6);
            {
                let o = open_mut(h);
                o.browse.set_scope(vf_core::browse::Scope::Client(i));
                o.browse.portal.reread();
            }
            h.run_steps(10);
        };

        // ==== K. Sharing documents through the portal.
        pick(&mut h, pi);
        select_in(&mut h, pi, "Income Tax/2025", "W-2");
        h.run_steps(10);
        wait_long(&mut h, "Share…", &|h| {
            h.query_by_role_and_label(Role::Button, "Share…").is_some()
        });
        h.get_by_role_and_label(Role::Button, "Share…")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Share…").union(button(&h, "Send secure link…"));
        shot(&mut h, "k02a-buttons", &[r]);
        h.get_by_role_and_label(Role::Button, "Share…").click();
        wait_long(&mut h, "the share window", &|h| said(h, "Invite someone…"));
        h.run_steps(6);
        dump(&h, "share");
        shot(&mut h, "k02b-share", &[]);
        let who = format!("{first} {last} ({p_email})");
        if h.query_by_label(&who).is_some() {
            h.get_by_label(&who).click();
            h.run_steps(4);
        }
        shot(&mut h, "k02c-ticked", &[]);
        for l in ["Save", "Share"] {
            if h.query_by_role_and_label(Role::Button, l).is_some() {
                h.get_by_role_and_label(Role::Button, l).click();
                break;
            }
        }
        h.run_steps(20);
        dump(&h, "shared");
        shot(&mut h, "k02d-shared", &[]);
        h.get_by_role_and_label(Role::Button, "Close window")
            .click();
        h.run_steps(6);
        // The client opens the portal on a phone and sends a file back.
        let back = t.path().join("2025 brokerage statement.pdf");
        let any_pdf = std::fs::read_dir(root.join(&folder).join("Income Tax/2025"))
            .unwrap()
            .flatten()
            .find(|e| e.path().extension().is_some_and(|x| x == "pdf"))
            .unwrap()
            .path();
        std::fs::copy(&any_pdf, &back).unwrap();
        node(&[&p_email, "train-shared", back.to_str().unwrap()]);
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        wait_long(&mut h, "the upload fetched", &|_| {
            std::fs::read_dir(&inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().contains("brokerage"))
        });
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(20);
        dump(&h, "intake");
        for _ in 0..600 {
            if h.query_by_role_and_label(Role::Button, "Refresh").is_some()
                && said(&h, "arrived while you were reviewing")
            {
                h.query_all_by_label("Refresh").last().unwrap().click();
            }
            if said(&h, "brokerage")
                && said(&h, "Ready")
                && !said(&h, "Looking for the client")
                && !said(&h, "Reading page text")
                && !said(&h, "arrived while you were reviewing")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        h.run_steps(10);
        shot(&mut h, "k05-intake", &[]);

        // ==== L. A secure link to someone outside the client.
        pick(&mut h, pi);
        select_in(&mut h, pi, "Income Tax/2025", "1040");
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "Send secure link…")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Send secure link…");
        shot(&mut h, "l02a-button", &[r]);
        h.get_by_role_and_label(Role::Button, "Send secure link…")
            .click();
        h.run_steps(10);
        dump(&h, "link");
        for (k, v) in [
            ("Their email", "lee.lender@example.com"),
            ("Their name", "Lee Lender"),
            ("Organization", "Example Bank"),
        ] {
            h.get_by_label(k).click();
            h.step();
            h.get_by_label(k).type_text(v);
            h.run_steps(2);
        }
        if h.query_all_by_label_contains("has consented to sending")
            .next()
            .is_some()
        {
            h.query_all_by_label_contains("has consented to sending")
                .next()
                .unwrap()
                .click();
            h.run_steps(3);
        }
        h.run_steps(4);
        shot(&mut h, "l02b-filled", &[]);
        h.get_by_role_and_label(Role::Button, "Send").click();
        h.run_steps(20);
        wait_long(&mut h, "sent", &|h| said(h, "Secure link sent to"));
        let r = text_rect(&h, "Secure link sent to");
        h.run_steps(6);
        dump(&h, "linksent");
        shot(&mut h, "l02c-sent", &[r]);
        h.get_by_role_and_label(Role::Button, "Close window")
            .click();
        h.run_steps(6);
        node(&["lee.lender@example.com", "train-secure"]);

        // ==== O. Balance owed….
        pick(&mut h, mi);
        wait_long(&mut h, "the panel", &|h| {
            h.query_by_role_and_label(Role::Button, "Balance owed…")
                .is_some()
        });
        let r = button(&h, "Balance owed…");
        shot(&mut h, "o02a-button", &[r]);
        h.get_by_role_and_label(Role::Button, "Balance owed…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "No balance on the portal.")
        });
        h.get_by_label("Balance amount").click();
        h.step();
        h.get_by_label("Balance amount").type_text("350");
        h.step();
        h.get_by_label("Balance memo").click();
        h.step();
        h.get_by_label("Balance memo")
            .type_text("2025 tax return preparation");
        h.run_steps(4);
        dump(&h, "balance");
        shot(&mut h, "o02b-filled", &[]);
        h.get_by_role_and_label(Role::Button, "Post to portal")
            .click();
        wait_long(&mut h, "posted", &|h| said(h, "Posted to the portal."));
        shot(&mut h, "o02c-posted", &[]);
        {
            let o = open_mut(&mut h);
            o.browse.portal.balance = None;
            o.browse.portal.look_at_balances_now();
        }
        wait_long(&mut h, "panel line due", &|h| {
            said(h, "$350.00 on the portal")
        });
        let r = text_rect(&h, "$350.00 on the portal");
        shot(&mut h, "o02d-panel", &[r]);
        node(&[&m_email, "accept"]);
        node(&[&m_email, "train-balance", &stripe]);
        open_mut(&mut h).browse.portal.fetch_now = true;
        wait_long(&mut h, "the receipt fetched", &|_| {
            std::fs::read_dir(&inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().contains("Payment receipt"))
        });
        {
            let o = open_mut(&mut h);
            o.browse.portal.look_at_balances_now();
        }
        h.run_steps(20);
        shot(&mut h, "o04a-paid-panel", &[]);
        admin_page(&mut h, "Balances");
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        h.run_steps(8);
        h.get_by_label("Paid this month").click();
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        dump(&h, "balances");
        shot(&mut h, "o05-balances", &[]);

        // ==== N. End engagement… (after an agreement the client signed and pays monthly).
        pick(&mut h, bi);
        wait_long(&mut h, "the panel", &|h| {
            h.query_by_role_and_label(Role::Button, "Service agreement…")
                .is_some()
        });
        h.get_by_role_and_label(Role::Button, "Service agreement…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "SERVICES AND FEES") && !said(h, "Working…")
        });
        h.run_steps(4);
        h.get_by_label("Bookkeeping").click();
        h.run_steps(3);
        h.get_by_label("Bookkeeping monthly fee").click();
        h.step();
        h.get_by_label("Bookkeeping monthly fee")
            .type_text("450.00");
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        open_mut(&mut h).browse.portal.agreement = None;
        h.run_steps(4);
        node(&["pat.sample@example.com", "agreement", &stripe]);
        pick(&mut h, bi);
        h.get_by_role_and_label(Role::Button, "End engagement…")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "End engagement…");
        shot(&mut h, "n02a-button", &[r]);
        h.get_by_role_and_label(Role::Button, "End engagement…")
            .click();
        h.run_steps(8);
        shot(&mut h, "n02b-window", &[]);
        for (k, v) in [
            ("[Date of Closure Letter]", "November 27, 2026"),
            ("[Date]", "November 27, 2026"),
            ("[Name of Engagement]", "Bookkeeping"),
            ("[Telephone Number]", "555-0100"),
            (
                "[and Specified Parties, optional if applicable]",
                "and its lender",
            ),
        ] {
            let Some(field) = h.query_all_by_label(k).last() else {
                continue;
            };
            field.click();
            h.step();
            h.query_all_by_label(k).last().unwrap().type_text(v);
            h.run_steps(2);
        }
        h.run_steps(4);
        let r = button(&h, "Send and end engagement…");
        shot(&mut h, "n02c-filled", &[r]);
        h.get_by_role_and_label(Role::Button, "Send and end engagement…")
            .click();
        h.run_steps(3);
        let r = button(&h, "End engagement");
        shot(&mut h, "n02d-ask", &[r]);
        h.get_by_role_and_label(Role::Button, "End engagement")
            .click();
        wait_long(&mut h, "ended", &|h| {
            said(h, "closure letter is on the client's portal")
        });
        let r = text_rect(&h, "closure letter is on the client's portal");
        shot(&mut h, "n02e-ended", &[r]);
        while h
            .query_by_role_and_label(Role::Button, "Close window")
            .is_some()
        {
            h.get_by_role_and_label(Role::Button, "Close window")
                .click();
            h.run_steps(4);
        }
        node(&["pat.sample@example.com", "train-signin", "", "closure"]);
        open_mut(&mut h).browse.portal.agreement = None;
        open_mut(&mut h).browse.portal.fetch_now = true;
        wait_long(&mut h, "the closure letter fetched", &|_| {
            std::fs::read_dir(&inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .contains("closure")
                })
        });
    }

    /// Owner 2026-11-27: the frames of the training video "Annual agreements and tiered pricing"
    /// (staff side) against a real Vibe Portal and the stand-in Stripe: Admin › Portal › Pricing
    /// plans (New plan…, the package grid, add-ons, As the client sees it, Save); Deliver… with
    /// Packages and add-ons; the partners and letter templates; Service agreement… sent to a
    /// business; the client signs and pays (off camera); Deliveries' Monthly charge failed; the
    /// monthly amount changed; next year's agreement sent from Admin › Portal › Agreements; End
    /// engagement…; the PDFs filed by Intake. Frames at 1920×1080 in `VF_WALK_DIR`, what to look
    /// at outlined in amber. The same `VF_WALK_*` variables as `agreement_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn training_agreements_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        for (_, name) in vf_portal::delivery::AGREEMENT_STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::AGREEMENT_FOLDER, true)
                .unwrap();
        }
        vf_fs::doctypes::ensure(
            &root,
            vf_portal::inbox::RECEIPT_DOC_TYPE,
            "Billing/{Year}",
            true,
        )
        .unwrap();
        vf_ingest::parsers::sync_filer_rules_report(&root).unwrap();
        vf_letters::store::seed_if_missing(&root).unwrap();
        // The firm has filled in its Terms and Conditions once (A17).
        let mut terms = vf_letters::store::load_all(&root)
            .unwrap()
            .into_iter()
            .find(|t| t.kind == vf_letters::TERMS)
            .unwrap();
        let mut json = serde_json::to_string(&terms.blocks).unwrap();
        for (k, v) in [
            ("[X]", "30"),
            ("[Portal Name]", "Sample Portal"),
            ("[State Name]", "Ohio"),
            (
                "[Name of State or Federal Court Desired]",
                "Franklin County, Ohio",
            ),
        ] {
            json = json.replace(k, v);
        }
        terms.blocks = serde_json::from_str(&json).unwrap();
        terms.sample = false;
        vf_letters::store::save(&root, &terms).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        // A frame, with what to look at outlined in amber.
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 120, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
                }
            };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };
        let text_rect = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.query_all_by_label_contains(l)
                .next()
                .unwrap_or_else(|| panic!("{l}: not shown"))
                .rect()
        };
        let type_into =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, v: &str| {
                h.get_by_label(label).click();
                h.step();
                h.key_press(egui::Key::End);
                for _ in 0..12 {
                    h.key_press(egui::Key::Backspace);
                    h.step();
                }
                h.get_by_label(label).type_text(v);
                h.run_steps(3);
            };
        let admin_page = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, page: &str| {
            h.get_by_role_and_label(Role::Button, "Admin").click();
            h.run_steps(6);
            h.get_by_role_and_label(Role::Button, "Portal").click();
            h.run_steps(6);
            h.get_all_by_label(page).next().unwrap().scroll_to_me();
            h.run_steps(4);
            h.get_all_by_label(page).next().unwrap().click();
            h.run_steps(8);
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        // A person (a 1040, priced from the plan) and a business (the agreement).
        let clients = open_mut(&mut h).browse.snap.clients.clone();
        let (pi, person) = clients
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.contains(", "))
            .map(|(i, c)| (i, c.clone()))
            .unwrap();
        let (bi, biz) = clients
            .iter()
            .enumerate()
            .find(|(i, _)| *i != pi)
            .map(|(i, c)| (i, c.clone()))
            .unwrap();
        let folder = biz.folder_rel.clone().unwrap();
        let first = person
            .name
            .split(", ")
            .nth(1)
            .unwrap_or("Jordan")
            .to_string();
        let last = person
            .name
            .split(", ")
            .next()
            .unwrap_or("Sample")
            .to_string();
        let p_email = format!(
            "{}.{}@example.com",
            first.to_lowercase(),
            last.to_lowercase()
        );
        vf_fs::contacts::set_by_hand(
            &root,
            &person.id,
            &vf_fs::contacts::Row {
                first: first.clone(),
                last: last.clone(),
                email: p_email.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        vf_fs::clients::set_entity_type(&root, &biz.id, Some("llc")).unwrap();
        for (k, v) in [
            ("first", "Pat"),
            ("last", "Sample"),
            ("email", "pat.sample@example.com"),
        ] {
            vf_fs::contacts::set_one_by_hand(&root, &biz.id, k, v).unwrap();
        }
        let p = crate::portal::client(&s);
        p.put_client(&person.id, &person.ext_id, &person.name, None)
            .unwrap();
        p.invite(
            &person.id,
            Some(&p_email),
            None,
            &format!("{first} {last}"),
            "Taxpayer",
        )
        .unwrap();
        p.put_client(&biz.id, &biz.ext_id, &biz.name, None).unwrap();
        p.invite(
            &biz.id,
            Some("pat.sample@example.com"),
            None,
            "Pat Sample",
            "Owner",
        )
        .unwrap();

        // ==== Part 1: tiered pricing.
        // ---- p02. Admin › Portal › Pricing plans, New plan….
        admin_page(&mut h, "Pricing plans");
        let r = button(&h, "New plan…");
        shot(&mut h, "p02a-pricing-plans", &[r]);
        h.get_by_role_and_label(Role::Button, "New plan…").click();
        h.run_steps(8);
        let top = text_rect(&h, "Return type").union(text_rect(&h, "Base fee name"));
        let top = egui::Rect::from_min_max(top.min, egui::pos2(top.max.x + 210.0, top.max.y));
        shot(&mut h, "p02b-new-plan", &[top]);
        // ---- p03. The package grid.
        let rec = h
            .query_all_by_label("Recommended")
            .map(|n| n.rect())
            .reduce(|a, b| a.union(b))
            .unwrap();
        let price = text_rect(&h, "Price over the return fee");
        let price = egui::Rect::from_min_max(
            egui::pos2(price.min.x, price.min.y - 30.0),
            egui::pos2(rec.max.x, rec.max.y),
        );
        shot(&mut h, "p03a-grid", &[price]);
        let r = button(&h, "Add package").union(button(&h, "Add service"));
        shot(&mut h, "p03b-grid-add", &[r]);
        // ---- p04. Add-ons.
        h.get_all_by_label("Add-ons").next().unwrap().click();
        h.run_steps(6);
        let r = text_rect(&h, "Offered unless unticked");
        shot(&mut h, "p04-addons", &[r]);
        h.get_all_by_label("Packages").next().unwrap().click();
        h.run_steps(6);
        // ---- p05. As the client sees it; Save; Copy to next year….
        let side = text_rect(&h, "As the client sees it");
        let side =
            egui::Rect::from_min_max(side.min, egui::pos2(side.min.x + 300.0, side.min.y + 350.0));
        assert!(said(&h, "$1,708.68"), "the Concierge minimum applies");
        shot(&mut h, "p05a-client-sees", &[side]);
        h.get_by_role_and_label(Role::Button, "Save").scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Save").union(button(&h, "Copy to next year…"));
        shot(&mut h, "p05b-save", &[r]);
        h.get_by_role_and_label(Role::Button, "Save").click();
        wait_long(&mut h, "saved", &|h| said(h, "Saved the 2025 1040 plan."));
        h.query_all_by_label_contains("Saved the 2025 1040 plan.")
            .next()
            .unwrap()
            .scroll_to_me();
        h.run_steps(4);
        let r = text_rect(&h, "Saved the 2025 1040 plan.");
        shot(&mut h, "p05c-saved", &[r]);

        // ---- p06. Deliver… with Packages and add-ons for the person.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(pi));
            o.browse.portal.reread();
        }
        h.run_steps(10);
        wait_long(&mut h, "the panel", &|h| {
            h.query_by_role_and_label(Role::Button, "Deliver…")
                .is_some()
        });
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| said(h, "WHAT HAS HAPPENED"));
        h.run_steps(6);
        h.get_by_label("Package and fee").scroll_to_me();
        h.run_steps(3);
        h.get_by_label("Package and fee").click();
        h.run_steps(4);
        h.get_by_label("Return fee").click();
        h.step();
        h.get_by_label("Return fee").type_text("1265.68");
        h.run_steps(4);
        h.get_by_label("Return fee").scroll_to_me();
        h.run_steps(3);
        let r = h.get_by_label("Return fee").rect();
        shot(&mut h, "p06a-deliver-packages", &[r]);
        type_into(&mut h, "Concierge price", "1700.00");
        h.run_steps(4);
        let r = h.get_by_label("Concierge price").rect();
        shot(&mut h, "p06b-deliver-override", &[r]);
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(6);

        // ==== Part 2: annual agreements.
        // ---- a07. Partners; the letter templates.
        admin_page(&mut h, "Agreements");
        if h.query_by_role_and_label(Role::Button, "Add partner")
            .is_none()
        {
            h.get_all_by_label("Partners (who signs for the firm)")
                .next()
                .unwrap()
                .click();
            h.run_steps(4);
        }
        h.get_by_role_and_label(Role::Button, "Add partner").click();
        h.run_steps(4);
        type_into(&mut h, "Partner name", "Morgan Example");
        h.get_by_role_and_label(Role::Button, "Save partners")
            .click();
        wait_long(&mut h, "partners saved", &|h| {
            said(h, "Saved for every PC.")
        });
        let r = h
            .get_by_label("Partner name")
            .rect()
            .union(button(&h, "Save partners"));
        let r = egui::Rect::from_min_max(r.min, egui::pos2(r.max.x + 260.0, r.max.y));
        shot(&mut h, "a07a-partners", &[r]);
        h.get_all_by_label("Letter templates")
            .next()
            .unwrap()
            .click();
        h.run_steps(8);
        if let Some(n) = h
            .query_all_by_label_contains("individual income tax return")
            .next()
        {
            n.click();
        }
        h.run_steps(8);
        shot(&mut h, "a07b-letter-templates", &[]);

        // ---- a08. Service agreement… for the business.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(bi));
            o.browse.portal.reread();
        }
        h.run_steps(10);
        wait_long(&mut h, "the panel", &|h| {
            h.query_by_role_and_label(Role::Button, "Service agreement…")
                .is_some()
        });
        let r = button(&h, "Service agreement…");
        shot(&mut h, "a08a-button", &[r]);
        h.get_by_role_and_label(Role::Button, "Service agreement…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "SERVICES AND FEES") && !said(h, "Working…")
        });
        h.run_steps(4);
        for (service, once, month) in [
            ("Bookkeeping", "", "450.00"),
            ("Payroll", "", "125.00"),
            ("Advisory", "500.00", ""),
        ] {
            h.get_by_label(service).click();
            h.run_steps(3);
            for (label, v) in [("one-time", once), ("monthly", month)] {
                if v.is_empty() {
                    continue;
                }
                let name = format!("{service} {label} fee");
                h.get_by_label(&name).click();
                h.step();
                h.get_by_label(&name).type_text(v);
                h.run_steps(2);
            }
        }
        h.run_steps(4);
        assert!(said(&h, "Total $500.00 one-time · $575.00 a month"));
        let year_r = text_rect(&h, "Agreement year");
        let year_r =
            egui::Rect::from_min_max(year_r.min, egui::pos2(year_r.max.x + 190.0, year_r.max.y));
        shot(&mut h, "a08b-year", &[year_r]);
        let fees = text_rect(&h, "SERVICES AND FEES").union(text_rect(&h, "Total $500.00"));
        let fees = egui::Rect::from_min_max(fees.min, egui::pos2(fees.min.x + 520.0, fees.max.y));
        shot(&mut h, "a08c-services", &[fees]);
        // ---- a09. Letters to sign; Preview letters….
        h.query_all_by_label_contains("LETTERS TO SIGN")
            .next()
            .unwrap()
            .scroll_to_me();
        h.run_steps(4);
        let letters = text_rect(&h, "LETTERS TO SIGN").union(text_rect(
            &h,
            "The Terms and Conditions are part of each letter",
        ));
        let letters = egui::Rect::from_min_max(
            letters.min,
            egui::pos2(letters.min.x + 640.0, letters.max.y),
        );
        let r = button(&h, "Preview letters…");
        shot(&mut h, "a09a-letters", &[letters, r]);
        // ---- a10. Signed by, payment, Send.
        let signed = text_rect(&h, "Signed by").union(text_rect(&h, "(for the business)"));
        let pay = text_rect(&h, "Pay on the portal").union(text_rect(&h, "Sign only"));
        let send = button(&h, "Send to client");
        shot(&mut h, "a10a-signers", &[signed, pay, send]);
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        let r = text_rect(&h, "Sent to the client.");
        shot(&mut h, "a10b-sent", &[r]);
        let a = p.agreements(&biz.id).unwrap();
        assert_eq!(a.len(), 1);
        let year = a[0].tax_year;
        open_mut(&mut h).browse.portal.agreement = None;
        h.run_steps(4);

        // ---- The client signs, sets up payment; a month is paid, the next fails (off camera).
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                "pat.sample@example.com",
                "agreement",
                &stripe,
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client's side");
        assert_eq!(p.delivery(&a[0].id).unwrap().status, "complete");

        // ---- a11. Admin › Portal › Deliveries: Kind, Monthly charge, Monthly charge failed.
        admin_page(&mut h, "Deliveries");
        h.get_by_label("Monthly charge failed").click();
        wait_long(&mut h, "the failed month", &|h| {
            said(h, &biz.name) && said(h, "Failed ")
        });
        h.run_steps(4);
        let r = text_rect(&h, "Failed ");
        let row = text_rect(&h, &biz.name);
        let row = egui::Rect::from_min_max(
            egui::pos2(row.min.x, row.min.y),
            egui::pos2(r.max.x, row.max.y.max(r.max.y)),
        );
        shot(&mut h, "a11b-failed", &[row]);

        // ---- a13. Next year's agreements from Admin › Portal › Agreements.
        admin_page(&mut h, "Agreements");
        {
            let a = open_mut(&mut h).admin.as_mut().unwrap();
            a.agreements.year = year + 1;
            a.agreements.reread();
        }
        let tick = format!("{} ({})", biz.name, biz.ext_id);
        wait_long(&mut h, "the bulk list", &|h| {
            h.query_by_label(&tick).is_some()
                && !said(h, "Reading the portal")
                && said(h, "Bookkeeping, Payroll, Advisory")
        });
        h.run_steps(4);
        h.get_by_label(&tick).click();
        h.run_steps(3);
        assert!(said(&h, "1 ticked"));
        let row = h.get_by_label(&tick).rect();
        let row = egui::Rect::from_min_max(row.min, egui::pos2(row.min.x + 1000.0, row.max.y));
        let go = text_rect(&h, "1 ticked").union(button(&h, "Send 1 agreement"));
        shot(&mut h, "a13a-bulk", &[row, go]);
        h.get_by_role_and_label(Role::Button, "Send 1 agreement")
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the run over", &|h| {
            h.query_by_role_and_label(Role::Button, "Stop").is_none()
        });
        h.run_steps(6);
        if !said(&h, "Sent 1 of 1 agreements.") {
            shot(&mut h, "a13x-bulk-not-sent", &[]);
            panic!("bulk not sent");
        }
        h.run_steps(30);
        let r = text_rect(&h, "Sent 1 of 1 agreements.");
        shot(&mut h, "a13b-bulk-sent", &[r]);

        // ---- a12. The monthly amount changed from the next charge.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Service agreement…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "Monthly charge") && !said(h, "Working…")
        });
        h.run_steps(4);
        type_into(&mut h, "New monthly amount", "600.00");
        let r = h
            .get_by_label("New monthly amount")
            .rect()
            .union(button(&h, "Change from the next charge"))
            .union(button(&h, "Stop monthly charge…"));
        shot(&mut h, "a12a-change", &[r]);
        h.get_by_role_and_label(Role::Button, "Change from the next charge")
            .click();
        wait_long(&mut h, "changed", &|h| said(h, "Changed to $600.00"));
        let r = text_rect(&h, "Changed to $600.00");
        shot(&mut h, "a12b-changed", &[r]);
        open_mut(&mut h).browse.portal.agreement = None;
        h.run_steps(4);

        // ---- a14. End engagement….
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        let r = button(&h, "End engagement…");
        h.get_by_role_and_label(Role::Button, "End engagement…")
            .click();
        h.run_steps(6);
        let _ = r;
        for (k, v) in [
            ("[Date of Closure Letter]", "November 27, 2026"),
            ("[Date]", "November 27, 2026"),
            ("[Name of Engagement]", "Bookkeeping, payroll and advisory"),
            ("[Telephone Number]", "555-0100"),
            (
                "[and Specified Parties, optional if applicable]",
                "and its lender",
            ),
        ] {
            let Some(field) = h.query_all_by_label(k).last() else {
                continue;
            };
            field.click();
            h.step();
            h.query_all_by_label(k).last().unwrap().type_text(v);
            h.run_steps(2);
        }
        h.run_steps(4);
        let r = button(&h, "Send and end engagement…");
        shot(&mut h, "a14a-end", &[r]);
        h.get_by_role_and_label(Role::Button, "Send and end engagement…")
            .click();
        h.run_steps(3);
        let r = button(&h, "End engagement");
        shot(&mut h, "a14b-end-ask", &[r]);
        h.get_by_role_and_label(Role::Button, "End engagement")
            .click();
        wait_long(&mut h, "ended", &|h| {
            said(h, "closure letter is on the client's portal")
        });
        let r = text_rect(&h, "closure letter is on the client's portal");
        shot(&mut h, "a14c-ended", &[r]);
        assert!(p.subscriptions(&biz.id).unwrap()[0].canceled_at.is_some());

        // ---- a15. The PDFs fetched and filed by Intake.
        open_mut(&mut h).browse.portal.agreement = None;
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let count = |inbox: &std::path::Path| {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .ends_with(".pdf")
                })
                .count()
        };
        let start = std::time::Instant::now();
        let mut asked = std::time::Instant::now();
        while count(&inbox) < 4 && start.elapsed().as_secs() < 90 {
            if asked.elapsed().as_secs() >= 10 {
                open_mut(&mut h).browse.portal.fetch_now = true;
                asked = std::time::Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.step();
        }
        let got: Vec<String> = std::fs::read_dir(&inbox)
            .map(|r| {
                r.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        eprintln!("Portal inbox: {got:?}");
        let n = count(&inbox);
        assert!(n >= 3, "the agreement PDFs fetched");
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        let commit = format!("Commit {n} files");
        wait_long(&mut h, "rows ready", &|h| said(h, &commit));
        let r = h
            .query_all_by_role_and_label(Role::Button, &commit)
            .last()
            .unwrap()
            .rect();
        shot(&mut h, "a15a-intake", &[r]);
        h.query_all_by_role_and_label(Role::Button, &commit)
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(10);
        let eng = root
            .join(&folder)
            .join("Correspondence")
            .join(year.to_string())
            .join("Engagement");
        let letters_pdf = eng.join(format!("{year} Agreement 1 - Letters.pdf"));
        wait_long(&mut h, "filed", &|_| letters_pdf.exists());
        // The signed letters, for the video's look at a letter as the client reads it.
        std::fs::copy(&letters_pdf, out.join("agreement-letters.pdf")).unwrap();
        h.state_mut().tab = crate::app::Tab::Browse;
        h.run_steps(10);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(bi));
        }
        h.run_steps(6);
        let want = format!("{year} Agreement 1 - Letters.pdf");
        let start = std::time::Instant::now();
        while !(said(&h, &want) && said(&h, "Monthly services")) {
            assert!(
                start.elapsed().as_secs() < 180,
                "the filed PDFs listed: never"
            );
            h.query_all_by_label("Refresh").last().unwrap().click();
            for _ in 0..50 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                h.step();
            }
        }
        h.run_steps(6);
        let filed: Vec<egui::Rect> = [
            want.clone(),
            format!("{year} Agreement 4 - Fee.pdf"),
            "Monthly services".to_string(),
        ]
        .iter()
        .filter_map(|n| h.query_all_by_label_contains(n).next().map(|x| x.rect()))
        .collect();
        let marks: Vec<egui::Rect> = filed
            .iter()
            .map(|r| egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 740.0, r.max.y)))
            .collect();
        shot(&mut h, "a15b-filed", &marks);
    }

    /// Owner 2026-11-27: the frames of the training videos "Filing documents with Intake" and
    /// "Intake rules and setup" (staff): an inbox of made-up files, one for each case Intake
    /// shows (ready, a name match, no client, two years, a form read with no client in its
    /// name, two statements to combine), filed, set in Just filed, undone; then a rule built
    /// from a file, the Rules screen and Setup. Frames at 1920×1080 in `VF_WALK_DIR`, what to
    /// look at outlined in amber. Needs no portal.
    #[test]
    #[ignore = "makes training frames"]
    fn training_intake_walkthrough() {
        let Ok(out) = std::env::var("VF_WALK_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        // The firm's scanner inbox.
        let inbox = t.path().join("Scans inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();
        // A W-2 from the made-up form set, given to one of this cabinet's people.
        let w2 = vf_extract::synthetic::cases()
            .into_iter()
            .find(|c| c.name.starts_with("w2 2025 ORTI3300"))
            .expect("a synthetic W-2")
            .text
            .replace("Ortiz, Sam", "Placeholt, Casey")
            .replace("Sam Ortiz", "Casey Placeholt")
            .replace("SAM ORTIZ", "CASEY PLACEHOLT");
        let w2_lines: Vec<&str> = w2.lines().collect();
        let page = |n: &str, lines: &[&str]| {
            crate::ui::test_util::write_lines_pdf(&inbox.join(n), &[lines]);
        };
        page("Placeholt, Casey_1002_2025 W-2.pdf", &w2_lines);
        page(
            "Synthetic Orchards LLC_1004_2025 1099-INT.pdf",
            &["Form 1099-INT Interest Income 2025", "Northwind Bank NA"],
        );
        page(
            "Example Freight LLC_1005_2025 Bank Statement January.pdf",
            &[
                "Northwind Bank NA",
                "Statement: January 2025",
                "Example Freight LLC",
            ],
        );
        page(
            "Example Freight LLC_1005_2025 Bank Statement February.pdf",
            &[
                "Northwind Bank NA",
                "Statement: February 2025",
                "Example Freight LLC",
            ],
        );
        page(
            "Placeholt Morgan 2025 1098.pdf",
            &[
                "Form 1098 Mortgage Interest Statement 2025",
                "Lakeside Mortgage Co",
            ],
        );
        page(
            "Newco LLC_NEWC2002_2025 W-2.pdf",
            &["Form W-2 Wage and Tax Statement 2025"],
        );
        page(
            "Placeholt, Jordan_1003_2020 Trust 2025 K-1.pdf",
            &["Schedule K-1 (Form 1041) 2025", "The Placeholt 2020 Trust"],
        );
        page(
            "Synthetic Orchards LLC_1004_2025 Property Tax Bill.pdf",
            &[
                "County Treasurer",
                "2025 Property Tax Bill",
                "Synthetic Orchards LLC",
            ],
        );
        page("scan0001.pdf", &w2_lines);
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 120, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let names: Vec<String> = open_mut(&mut h)
            .browse
            .snap
            .clients
            .iter()
            .map(|c| format!("{} ({})", c.name, c.ext_id))
            .collect();
        eprintln!("clients: {names:?}");

        // ---- i01. The Intake tab and its count.
        shot(&mut h, "i01-browse", &[]);
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        wait_long(&mut h, "the rows planned", &|h| {
            said(h, "scan0001.pdf")
                && said(h, "Newco LLC_NEWC2002_2025 W-2.pdf")
                && h.query_by_label("read").is_some()
        });
        h.run_steps(20);
        let text_of = |h: &egui_kittest::Harness<'static, VibeFilesApp>, t: &str| {
            h.query_all_by_label(t)
                .next()
                .or_else(|| h.query_all_by_label_contains(t).next())
                .unwrap_or_else(|| panic!("{t}: not shown"))
                .rect()
        };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };
        let type_into =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, v: &str| {
                h.get_by_label(label).click();
                h.step();
                h.key_press(egui::Key::End);
                for _ in 0..40 {
                    h.key_press(egui::Key::Backspace);
                }
                h.step();
                h.get_by_label(label).type_text(v);
                h.run_steps(3);
            };
        let r = h
            .query_all_by_label_contains("Intake (")
            .next()
            .unwrap()
            .rect();
        shot(&mut h, "i02-queue", &[r]);
        // The scanning and inboxes panel hidden, so the queue has the room.
        h.get_by_label("Hide").click();
        h.run_steps(8);
        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
            }
        };
        let r = [
            "File",
            "Date",
            "Client",
            "Rule",
            "Year",
            "Destination",
            "Status",
        ]
        .iter()
        .map(|c| h.get_by_label(&format!("Sort by {c}")).rect())
        .reduce(|a, b| a.union(b))
        .unwrap();
        shot(&mut h, "i03-columns", &[r]);

        // ---- i04. The preview pane, beside the list.
        h.get_by_label("Preview pane").click();
        h.run_steps(6);
        h.query_all_by_label_contains("Placeholt, Casey_1002_2025 W-2.pdf")
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node().role() == Role::Label
            })
            .unwrap()
            .click();
        h.run_steps(30);
        let r = h.get_by_label("Preview pane").rect();
        shot(&mut h, "i04-preview-pane", &[r]);
        h.get_by_label("Preview pane").click();
        h.run_steps(6);

        // ---- i05. No client: New client….
        let newco = "Newco LLC_NEWC2002_2025 W-2.pdf";
        h.get_by_label(&format!("New client for {newco}"))
            .scroll_to_me();
        h.run_steps(4);
        let r = h
            .get_by_label(&format!("New client for {newco}"))
            .rect()
            .union(text_of(&h, "ID NEWC2002 not in the client list"));
        shot(&mut h, "i05a-needs-client", &[r]);
        h.get_by_label(&format!("New client for {newco}")).click();
        wait_long(&mut h, "the New client dialog", &|h| {
            h.query_by_label("Create client (one transaction)")
                .is_some()
        });
        h.run_steps(4);
        let r = button(&h, "Create client (one transaction)");
        shot(&mut h, "i05b-new-client", &[r]);
        h.get_by_label("Create client (one transaction)").click();
        h.run_steps(4);
        wait_long(&mut h, "Newco's row given its client", &|h| {
            h.query_by_label(&format!("New client for {newco}"))
                .is_none()
        });
        h.run_steps(10);

        // ---- i06. A name match offered.
        let morgan = "Placeholt Morgan 2025 1098.pdf";
        h.get_by_label("Use Placeholt, Morgan (1001)")
            .scroll_to_me();
        h.run_steps(4);
        let r = h
            .get_by_label("Use Placeholt, Morgan (1001)")
            .rect()
            .union(text_of(&h, "No ID in filename"));
        shot(&mut h, "i06a-suggested", &[r]);
        h.get_by_label("Use Placeholt, Morgan (1001)").click();
        h.run_steps(4);
        wait_long(&mut h, "Morgan's row given its client", &|h| {
            h.query_by_label("Use Placeholt, Morgan (1001)").is_none()
        });
        h.run_steps(10);
        h.get_by_label(&format!("Rename {morgan}")).scroll_to_me();
        h.run_steps(4);
        let r = h.get_by_label(&format!("Rename {morgan}")).rect();
        shot(&mut h, "i06b-matched", &[r]);

        // ---- i07. Two years in the name: the row asks for the year.
        let k1 = "Placeholt, Jordan_1003_2020 Trust 2025 K-1.pdf";
        let year = format!("Year for {k1}");
        h.get_by_label(&year).scroll_to_me();
        h.run_steps(4);
        dump(&h, "k1");
        let r = h.get_by_label(&year).rect();
        shot(&mut h, "i07a-needs-year", &[r]);
        h.get_by_label(&year).click();
        h.run_steps(4);
        dump(&h, "k1-open");
        if let Some(n) = h.query_all_by_role_and_label(Role::Button, "2025").last() {
            n.click();
        }
        h.run_steps(10);
        let r = h.get_by_label(&year).rect();
        shot(&mut h, "i07b-year-set", &[r]);

        // ---- i08. A form read for you: choose whose it is.
        let scan = "scan0001.pdf";
        h.get_by_label(&format!("Year for {scan}")).scroll_to_me();
        h.run_steps(4);
        let r = text_of(&h, "read").union(h.get_by_label(&format!("Year for {scan}")).rect());
        shot(&mut h, "i08a-read", &[r]);
        h.query_all_by_role_and_label(Role::Button, "Choose client…")
            .last()
            .unwrap()
            .click();
        h.run_steps(8);
        h.get_by_label("Search by name or ID").click();
        h.step();
        // The box starts with the file's name in it.
        h.key_press(egui::Key::End);
        for _ in 0..24 {
            h.key_press(egui::Key::Backspace);
        }
        h.step();
        h.get_by_label("Search by name or ID").type_text("Casey");
        h.run_steps(6);
        let casey = h
            .query_all_by_role(Role::Button)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Placeholt, Casey (1002)"))
            })
            .expect("Casey in the picker")
            .rect();
        let search = h.get_by_label("Search by name or ID").rect();
        shot(&mut h, "i08b-picker", &[search, casey]);
        h.query_all_by_role(Role::Button)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Placeholt, Casey (1002)"))
            })
            .unwrap()
            .click();
        h.run_steps(4);
        wait_long(&mut h, "the scan's row given Casey", &|h| {
            h.query_by_label(&format!("Rename {scan}")).is_some()
        });
        h.run_steps(10);
        h.get_by_label(&format!("Rename {scan}")).scroll_to_me();
        h.run_steps(4);
        let r = h
            .get_by_label(&format!("Rename {scan}"))
            .rect()
            .union(text_of(&h, "read"));
        shot(&mut h, "i08c-read-filed", &[r]);

        // ---- i09. The row menu.
        let jan = "Example Freight LLC_1005_2025 Bank Statement January.pdf";
        let feb = "Example Freight LLC_1005_2025 Bank Statement February.pdf";
        h.get_by_label(&format!("Select {jan}")).scroll_to_me();
        h.run_steps(4);
        h.query_all_by_label(jan)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node().role() == Role::Label
            })
            .unwrap()
            .click_secondary();
        h.run_steps(6);
        dump(&h, "menu");
        shot(&mut h, "i09-row-menu", &[]);
        // Closed by a click on the queue's heading.
        h.query_all_by_label("Queue").next().unwrap().click();
        h.run_steps(6);
        if let Some(b) = h.query_by_role_and_label(Role::Button, "Untick all") {
            b.click();
            h.run_steps(4);
        }

        // ---- i10. Two statements combined into one PDF.
        h.get_by_label(&format!("Select {jan}")).click();
        h.run_steps(6);
        h.get_by_label(&format!("Select {feb}")).click();
        h.run_steps(6);
        let r = button(&h, "Combine")
            .union(h.get_by_label(&format!("Select {jan}")).rect())
            .union(h.get_by_label(&format!("Select {feb}")).rect());
        shot(&mut h, "i10a-tick-two", &[r]);
        h.get_by_role_and_label(Role::Button, "Combine").click();
        h.run_steps(10);
        let r = h
            .get_by_label("OUTPUT NAME")
            .rect()
            .union(button(&h, "Combine · replaces 2 rows"));
        shot(&mut h, "i10b-combine", &[r]);
        h.get_by_role_and_label(Role::Button, "Combine · replaces 2 rows")
            .click();
        wait_long(&mut h, "combined", &|h| said(h, "Combined"));
        h.run_steps(10);
        if let Some(b) = h.query_by_role_and_label(Role::Button, "Untick all") {
            b.click();
            h.run_steps(4);
        }
        let r = text_of(&h, "Combined");
        shot(&mut h, "i10c-combined", &[r]);

        // ---- i11. Tick ready, Commit, the summary.
        h.get_by_role_and_label(Role::Button, "Tick ready").click();
        h.run_steps(6);
        let commit = h
            .query_all_by_role(Role::Button)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Commit "))
            })
            .expect("the Commit button")
            .rect();
        let r = button(&h, "Tick ready");
        shot(&mut h, "i11a-tick-ready", &[r, commit]);
        h.query_all_by_role(Role::Button)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Commit "))
            })
            .unwrap()
            .click();
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.run_steps(6);
        dump(&h, "summary");
        shot(&mut h, "i11b-summary", &[]);
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        wait_long(&mut h, "Just filed", &|h| said(h, "Just filed"));
        h.run_steps(20);
        // Already in the cabinet: the scan was Casey's W-2 again. Keep both (a removed copy
        // would make the batch's Undo unavailable).
        let r = text_of(&h, "Already in the cabinet").union(button(&h, "Replace old"));
        shot(&mut h, "i12a-already", &[r]);
        h.get_by_role_and_label(Role::Button, "Keep both").click();
        h.run_steps(10);
        // Just filed: the statement's doc type.
        let combos = |h: &egui_kittest::Harness<'static, VibeFilesApp>| -> Vec<egui::Rect> {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::ComboBox)
                .filter(|n| {
                    n.accesskit_node()
                        .value()
                        .is_some_and(|v| v == "Needs doc type")
                })
                .map(|n| n.rect())
                .collect()
        };
        let cs = combos(&h);
        eprintln!("doc type boxes: {}", cs.len());
        let r = text_of(&h, "Just filed:");
        shot(&mut h, "i12b-just-filed", &[r]);
        {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::ComboBox)
                .filter(|n| {
                    n.accesskit_node()
                        .value()
                        .is_some_and(|v| v == "Needs doc type")
                })
                .nth(1)
                .unwrap()
                .click();
        }
        h.run_steps(6);
        h.get_by_label("Type to search").click();
        h.step();
        h.get_by_label("Type to search").type_text("statement");
        h.run_steps(6);
        dump(&h, "doctypes");
        let pick = {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::Button)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l == "Bank statement")
                })
                .map(|n| n.rect())
        };
        let search = h.get_by_label("Type to search").rect();
        shot(&mut h, "i12c-doc-type", &[search]);
        if pick.is_some() {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::Button)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l == "Bank statement")
                })
                .unwrap()
                .click();
        }
        h.run_steps(8);
        let apply = h
            .query_all_by_role(Role::Button)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Apply ("))
            })
            .unwrap()
            .rect();
        shot(&mut h, "i12d-apply", &[apply]);
        h.query_all_by_role(Role::Button)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Apply ("))
            })
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "applied", &|h| {
            h.query_by_role_and_label(Role::Button, "Apply (0)")
                .is_some()
        });
        h.run_steps(6);
        dump(&h, "applied");
        shot(&mut h, "i12e-applied", &[]);

        // ---- i13. Undo last batch, on History.
        h.get_by_role_and_label(Role::Button, "History").click();
        h.run_steps(10);
        dump(&h, "history");
        let r = button(&h, "Undo last batch…");
        shot(&mut h, "i13a-history", &[r]);
        h.get_by_role_and_label(Role::Button, "Undo last batch…")
            .click();
        h.run_steps(6);
        let r = button(&h, "Confirm undo");
        shot(&mut h, "i13b-confirm-undo", &[r]);
        h.get_by_role_and_label(Role::Button, "Confirm undo")
            .click();
        wait_long(&mut h, "undone", &|_| {
            inbox.join("Placeholt, Casey_1002_2025 W-2.pdf").exists()
        });
        h.run_steps(20);
        shot(&mut h, "i13c-undone", &[]);
        if let Some(b) = h.query_by_role_and_label(Role::Button, "Close") {
            b.click();
            h.run_steps(4);
        }

        // ==== The rules and setup video.
        // ---- r01. Build a rule from a file the catch-all took.
        h.get_by_role_and_label(Role::Button, "Preview").click();
        let bill = "Synthetic Orchards LLC_1004_2025 Property Tax Bill.pdf";
        wait_long(&mut h, "the bill back in the queue", &|h| {
            h.query_by_label(&format!("Select {bill}")).is_some()
        });
        h.run_steps(10);
        h.get_by_label(&format!("Select {bill}")).scroll_to_me();
        h.run_steps(4);
        let r = text_of(&h, "Catch-all");
        shot(&mut h, "r01a-catch-all", &[r]);
        h.query_all_by_label(bill)
            .find(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node().role() == Role::Label
            })
            .unwrap()
            .click_secondary();
        h.run_steps(6);
        let r = button(&h, "Build a rule from this file…");
        shot(&mut h, "r01b-build-menu", &[r]);
        h.get_by_role_and_label(Role::Button, "Build a rule from this file…")
            .click();
        h.run_steps(12);
        // ---- r02. The Rules screen: the test name, then Build a rule.
        h.get_by_label("Test a filename").scroll_to_me();
        h.run_steps(6);
        let r = h
            .get_by_label("Test a filename")
            .rect()
            .union(text_of(&h, "MATCHED RULE"));
        shot(&mut h, "r02a-test-name", &[r]);
        h.get_by_role_and_label(Role::Button, "All of").click();
        h.run_steps(3);
        for w in ["Property", "Tax", "Bill"] {
            h.get_by_label(&format!("Word: {w}")).click();
            h.run_steps(3);
        }
        h.get_by_label("RULE NAME").scroll_to_me();
        h.run_steps(4);
        type_into(&mut h, "RULE NAME", "Property tax bills");
        h.get_by_label("FOLDER").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Income Tax/{Year}")
            .click();
        h.run_steps(6);
        h.get_by_label("Create rule").scroll_to_me();
        h.run_steps(4);
        let words = text_of(&h, "CONTAINS ALL OF");
        let words =
            egui::Rect::from_min_max(words.min, egui::pos2(words.min.x + 600.0, words.max.y));
        let r = h
            .get_by_label("RULE NAME")
            .rect()
            .union(h.get_by_label("FOLDER").rect())
            .union(h.get_by_label("FILE NAME").rect())
            .union(button(&h, "Create rule"));
        shot(&mut h, "r02b-build", &[words, r]);
        h.get_by_role_and_label(Role::Button, "Create rule").click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "Save").scroll_to_me();
        h.run_steps(4);
        let r = h
            .get_by_label("PRIMARY DESTINATION")
            .rect()
            .union(button(&h, "Save"));
        let added = text_of(&h, "added above Catch-all");
        shot(&mut h, "r03a-new-rule", &[r, added]);
        h.get_by_role_and_label(Role::Button, "Save").click();
        wait_long(&mut h, "rules saved", &|h| said(h, "Rules saved."));
        h.run_steps(6);
        h.get_by_label("Test a filename").scroll_to_me();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Test").click();
        h.run_steps(8);
        let r = text_of(&h, "MATCHED RULE");
        let r = egui::Rect::from_min_max(
            egui::pos2(r.min.x, r.min.y - 4.0),
            egui::pos2(r.min.x + 760.0, r.max.y + 34.0),
        );
        shot(&mut h, "r03b-tested", &[r]);
        // The row in Preview now goes by the new rule.
        h.get_by_role_and_label(Role::Button, "Preview").click();
        wait_long(&mut h, "the bill by its new rule", &|h| {
            h.query_by_role_and_label(Role::Button, "Property tax bills")
                .is_some()
        });
        h.run_steps(10);
        h.get_by_label(&format!("Rename {bill}")).scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Property tax bills")
            .union(h.get_by_label(&format!("Rename {bill}")).rect());
        shot(&mut h, "r04-row-new-rule", &[r]);

        // ---- r05. Setup.
        h.get_by_role_and_label(Role::Button, "Setup").click();
        h.run_steps(12);
        shot(&mut h, "r05a-setup", &[]);
        h.get_by_label("Include sub-folders").scroll_to_me();
        h.run_steps(6);
        let r = text_of(&h, "Inboxes").union(h.get_by_label("Include sub-folders").rect());
        let r = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 900.0, r.max.y));
        shot(&mut h, "r05b-inboxes", &[r]);
        h.get_by_role_and_label(Role::Button, "Choose CSV…")
            .scroll_to_me();
        h.run_steps(6);
        let r = text_of(&h, "Client master list").union(button(&h, "Choose CSV…"));
        shot(&mut h, "r05c-client-list", &[r]);
        h.get_by_label("ID PATTERN").scroll_to_me();
        h.run_steps(6);
        let r = h
            .get_by_label("ID PATTERN")
            .rect()
            .union(h.get_by_label("SUGGEST AT").rect());
        shot(&mut h, "r05d-id-pattern", &[r]);
        h.get_by_role_and_label(Role::Button, "Reset to default")
            .scroll_to_me();
        h.run_steps(6);
        let r = button(&h, "{Client} ({ID})")
            .union(button(&h, "Not Yet Filed"))
            .union(button(&h, "Reset to default"));
        shot(&mut h, "r05e-template", &[r]);
    }

    /// Owner 2026-11-27: the frames of the training videos "Finding your way in Browse" and
    /// "Working in the Viewer" (staff): a made-up cabinet with doc types set as a firm that
    /// has used it, the tree's views, the list, details, preview, Find documents…, the row
    /// menu, a move with Undo; then a return in the Viewer: pages, find, annotations,
    /// tickmarks, stamps, page tools, Save and Close. Frames at 1920×1080 in `VF_WALK_DIR`,
    /// what to look at outlined in amber. Run it as a made-up login (`USERNAME=ppreparer`): the
    /// Viewer's initials and their message come from the Windows login.
    #[test]
    #[ignore = "makes training frames"]
    fn training_browse_viewer_walkthrough() {
        let Ok(out) = std::env::var("VF_WALK_DIR") else {
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
        // A return of several pages to view.
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') && name.contains(", ") {
                let dir = e.path().join("Income Tax").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join("2025 Form 1040 Return - Sample.pdf"),
                    crate::ui::test_util::SAMPLE_RETURN,
                );
            }
        }
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            if let Some(b) = h.query_by_role_and_label(Role::Button, "Dismiss") {
                b.click();
            }
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
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
        h.run_steps(12);

        let text_of = |h: &egui_kittest::Harness<'static, VibeFilesApp>, t: &str| {
            h.query_all_by_label(t)
                .next()
                .or_else(|| h.query_all_by_label_contains(t).next())
                .unwrap_or_else(|| panic!("{t}: not shown"))
                .rect()
        };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };
        if let Some(b) = h.query_by_role_and_label(Role::Button, "Dismiss") {
            b.click();
            h.run_steps(4);
        }

        // ---- b02. The layout: the tree, the list, the details.
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        wait_for(&mut h, |h| h.query_by_label("Open in Viewer").is_some());
        std::thread::sleep(std::time::Duration::from_millis(500));
        h.run_steps(10);
        let r1 = button(&h, "Hide folders");
        let r2 = button(&h, "Hide details");
        shot(&mut h, "b02-layout", &[r1, r2]);

        // ---- b03. The tree: views, then By year.
        let views = button(&h, "Recent").union(button(&h, "Folders with no client ID"));
        let views =
            egui::Rect::from_min_max(views.min, egui::pos2(views.min.x + 300.0, views.max.y));
        let mode = button(&h, "Folders").union(button(&h, "By year"));
        shot(&mut h, "b03a-views", &[views, mode]);
        h.get_by_role_and_label(Role::Button, "Needs doc type")
            .click();
        h.run_steps(12);
        let r = button(&h, "Needs doc type");
        shot(&mut h, "b03b-needs-doc-type", &[r]);
        h.get_by_role_and_label(Role::Button, "By year").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Expand Demoreaux, Avery  1003")
            .click();
        h.run_steps(8);
        dump(&h, "byyear");
        let r = button(&h, "By year");
        shot(&mut h, "b03c-by-year", &[r]);
        h.get_by_role_and_label(Role::Button, "Folders").click();
        h.run_steps(6);

        // ---- b04. The list: sort, columns.
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "DOC TYPE").click();
        h.run_steps(8);
        let heads = ["NAME", "CLIENT", "DOC TYPE", "YEAR", "STATUS"]
            .iter()
            .filter_map(|c| h.query_by_role_and_label(Role::Button, c).map(|n| n.rect()))
            .reduce(|a, b| a.union(b))
            .unwrap();
        let cols = button(&h, "Columns…");
        shot(&mut h, "b04a-sorted", &[heads, cols]);

        // ---- b05. One document's details: the return has no doc type yet.
        select_in(&mut h, 0, "Income Tax/2025", "Sample");
        h.run_steps(12);
        h.get_by_role_and_label(Role::ComboBox, "DOC TYPE")
            .scroll_to_me();
        h.run_steps(4);
        let r = h
            .get_by_role_and_label(Role::ComboBox, "DOC TYPE")
            .rect()
            .union(h.get_by_label("TAGS").rect());
        shot(&mut h, "b05a-details", &[r]);
        {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::ComboBox)
                .find(|n| n.accesskit_node().label().is_some_and(|l| l == "DOC TYPE"))
                .unwrap()
                .click();
        }
        h.run_steps(6);
        h.get_by_label("Type to search").click();
        h.step();
        h.get_by_label("Type to search").type_text("return copy");
        h.run_steps(6);
        let r = h.get_by_label("Type to search").rect();
        shot(&mut h, "b05b-doc-type-list", &[r]);
        h.get_by_role_and_label(Role::Button, "Return copy — federal")
            .click();
        h.run_steps(10);
        let r = h
            .get_by_role_and_label(Role::ComboBox, "DOC TYPE")
            .rect()
            .union(h.get_by_role_and_label(Role::ComboBox, "STATUS").rect());
        let apply = {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::Button)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l.starts_with("Apply to "))
                })
                .expect("Apply to …")
                .rect()
        };
        shot(&mut h, "b05c-doc-type-set", &[r, apply]);
        {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::Button)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l.starts_with("Apply to "))
                })
                .unwrap()
                .click();
        }
        h.run_steps(12);
        let r = text_of(&h, "Return copy — fed");
        shot(&mut h, "b05d-applied", &[r]);

        // ---- b06. The preview, and Open in Viewer.
        let r = button(&h, "Open in Viewer").union(button(&h, "Open in program"));
        shot(&mut h, "b06-preview", &[r]);

        // ---- b07. Find documents….
        h.get_by_role_and_label(Role::Button, "Find documents…")
            .click();
        h.run_steps(6);
        dump(&h, "find");
        h.get_by_label("Words").click();
        h.step();
        h.get_by_label("Words").type_text("1099");
        h.run_steps(4);
        let r = h.get_by_label("Words").rect();
        shot(&mut h, "b07a-find", &[r]);
        h.get_by_label("Find").click();
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(400));
            h.run_steps(10);
        }
        let r = text_of(&h, "documents · 0 selected");
        shot(&mut h, "b07b-found", &[r]);
        h.get_by_role_and_label(Role::Button, "Save search…")
            .click();
        h.run_steps(6);
        h.get_by_label("Name").click();
        h.step();
        h.get_by_label("Name").type_text("1099s, all clients");
        h.run_steps(4);
        let r = h.get_by_label("Name").rect().union(button(&h, "Save"));
        shot(&mut h, "b07c-save-search", &[r]);
        h.get_by_role_and_label(Role::Button, "Save").click();
        h.run_steps(8);
        let r = text_of(&h, "1099s, all clients");
        shot(&mut h, "b07d-saved", &[r]);
        h.get_by_role_and_label(Role::Button, "Close").click();
        h.run_steps(6);
        // The saved search, opened from the tree.
        h.get_by_role_and_label(Role::Button, "1099s, all clients")
            .click();
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(400));
            h.run_steps(10);
        }
        dump(&h, "savedopen");
        shot(&mut h, "b07e-saved-open", &[]);

        // ---- b08. The selected document's actions; Pin.
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        h.run_steps(10);
        let r = button(&h, "Pin")
            .union(button(&h, "Open in Explorer"))
            .union(button(&h, "Copy path"))
            .union(button(&h, "Rename…"));
        shot(&mut h, "b08a-actions", &[r]);
        h.get_by_role_and_label(Role::Button, "Pin").click();
        h.run_steps(10);
        let r = button(&h, "Pinned");
        shot(&mut h, "b08b-pinned", &[r]);
        // ---- b09. Moving: a folder in the tree, and Undo last.
        let r = button(&h, "Income Tax").union(button(&h, "Correspondence"));
        let r2 = button(&h, "Undo last");
        shot(&mut h, "b09-move", &[r, r2]);

        // ---- b10. One document's details, further down.
        h.get_by_label("TAGS").scroll_to_me();
        h.run_steps(6);
        dump(&h, "details");
        if let Some(n) = h.query_all_by_label_contains("SHA-256").next() {
            n.scroll_to_me();
        }
        h.run_steps(6);
        shot(&mut h, "b10-more-details", &[]);

        // ==== The Viewer.
        // ---- v02. Opening: the return, from the preview.
        select_in(&mut h, 0, "Income Tax/2025", "Sample");
        wait_for(&mut h, |h| h.query_by_label("Open in Viewer").is_some());
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "Open in Viewer")
            .scroll_to_me();
        h.run_steps(6);
        let r = button(&h, "Open in Viewer");
        shot(&mut h, "v02a-open", &[r]);
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
        // The made-up staff member's initials (the box starts with this PC's login's).
        h.get_by_label("Signed").click();
        h.step();
        h.key_press(egui::Key::End);
        for _ in 0..8 {
            h.key_press(egui::Key::Backspace);
        }
        h.step();
        h.get_by_label("Signed").type_text("PP");
        h.run_steps(4);
        let r = button(&h, "Page 1 thumbnail").union(button(&h, "Page 4 thumbnail"));
        shot(&mut h, "v02b-viewer", &[r]);

        // ---- v03. Moving around.
        h.get_by_role_and_label(Role::Button, "Next page").click();
        h.run_steps(10);
        let r = button(&h, "Previous page")
            .union(h.get_by_label("Go to").rect())
            .union(button(&h, "Next page"));
        shot(&mut h, "v03a-pages", &[r]);
        let r = button(&h, "−")
            .union(button(&h, "+"))
            .union(button(&h, "Fit width"))
            .union(button(&h, "Rotate view"))
            .union(h.get_by_label("Continuous").rect());
        shot(&mut h, "v03b-view", &[r]);

        // ---- v04. Finding words.
        h.get_by_label("Find").click();
        h.step();
        h.get_by_label("Find").type_text("TOTAL");
        h.key_press(egui::Key::Enter);
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            h.run_steps(8);
        }
        dump(&h, "find");
        let r = h.get_by_label("Find").rect();
        shot(&mut h, "v04-find", &[r]);

        // Page 2 alone, for the annotations.
        h.get_by_label("Continuous").click();
        h.run_steps(4);
        h.get_by_label("Go to").click();
        h.step();
        h.key_press(egui::Key::End);
        h.key_press(egui::Key::Backspace);
        h.key_press(egui::Key::Backspace);
        h.get_by_label("Go to").type_text("2");
        h.key_press(egui::Key::Enter);
        h.run_steps(12);
        let drag = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    from: egui::Pos2,
                    to: egui::Pos2| {
            h.event(egui::Event::PointerMoved(from));
            h.step();
            h.event(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
            for k in 1..=8 {
                let t = k as f32 / 8.0;
                h.event(egui::Event::PointerMoved(from.lerp(to, t)));
                h.step();
            }
            h.event(egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(6);
        };
        // ---- v07. Page tools (before anything is unsaved: Pages… waits for a Save).
        h.get_by_role_and_label(Role::Button, "Pages…").click();
        h.run_steps(8);
        let r = h
            .get_by_role_and_label(Role::TextInput, "Pages")
            .rect()
            .union(button(&h, "Split into documents"));
        shot(&mut h, "v07-pages", &[r]);
        h.query_all_by_role_and_label(Role::Button, "Close")
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        assert!(
            h.query_by_label("Split into documents").is_none(),
            "the Pages window closed"
        );
        let click_at = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, at: egui::Pos2| {
            h.event(egui::Event::PointerMoved(at));
            h.step();
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(6);
        };
        // ---- v05. Annotating: a highlight.
        h.get_by_label("Highlight").click();
        h.run_steps(3);
        drag(&mut h, egui::pos2(188.0, 305.0), egui::pos2(486.0, 321.0));
        let r = egui::Rect::from_min_max(egui::pos2(180.0, 298.0), egui::pos2(494.0, 328.0));
        let r2 = h.get_by_label("Highlight").rect();
        shot(&mut h, "v05a-highlight", &[r, r2]);
        // A tickmark beside the wages.
        h.get_by_role_and_label(Role::Button, "Check").click();
        h.run_steps(3);
        click_at(&mut h, egui::pos2(500.0, 313.0));
        let r = egui::Rect::from_min_max(egui::pos2(486.0, 300.0), egui::pos2(520.0, 328.0));
        let r2 = button(&h, "Check");
        shot(&mut h, "v06a-tickmark", &[r, r2]);
        // A note.
        h.get_by_label("Note").click();
        h.run_steps(3);
        click_at(&mut h, egui::pos2(560.0, 340.0));
        h.get_by_role_and_label(Role::MultilineTextInput, "Text")
            .click();
        h.step();
        h.get_by_role_and_label(Role::MultilineTextInput, "Text")
            .type_text("Agrees to the W-2 on file.");
        h.run_steps(4);
        let r = h
            .get_by_role_and_label(Role::MultilineTextInput, "Text")
            .rect()
            .union(button(&h, "Add"));
        shot(&mut h, "v05b-note", &[r]);
        h.get_by_role_and_label(Role::Button, "Add").click();
        h.run_steps(8);
        shot(&mut h, "v05c-noted", &[]);
        // ---- Stamps.
        h.get_by_role_and_label(Role::Button, "Stamps").click();
        h.run_steps(6);
        h.get_by_label("Reviewed {initials} {date}").click();
        h.run_steps(4);
        let r = h
            .get_by_label("Reviewed {initials} {date}")
            .rect()
            .union(button(&h, "Stamp this page"));
        shot(&mut h, "v06b-stamps", &[r]);
        h.get_by_role_and_label(Role::Button, "Stamp this page")
            .click();
        h.run_steps(10);
        shot(&mut h, "v06c-stamped", &[]);

        // ---- v08. Save: a new version, the old one kept.
        let r = button(&h, "Save").union(button(&h, "Save as…"));
        let unsaved = text_of(&h, "unsaved");
        shot(&mut h, "v08a-save", &[r, unsaved]);
        h.get_by_role_and_label(Role::Button, "Save").click();
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(400));
            h.run_steps(10);
        }
        dump(&h, "saved");
        shot(&mut h, "v08b-saved", &[]);

        // ---- v09. Close.
        let r = h
            .query_all_by_role_and_label(Role::Button, "Close")
            .next()
            .unwrap()
            .rect();
        shot(&mut h, "v09-close", &[r]);
        h.query_all_by_role_and_label(Role::Button, "Close")
            .next()
            .unwrap()
            .click();
        h.run_steps(12);
        dump(&h, "closed");
        shot(&mut h, "v09b-closed", &[]);
    }

    /// Owner 2026-11-27: the frames of the training video "Scanning, cover sheets and print
    /// capture" (staff): the Scans folder and scanner settings on Intake, a cover sheet printed
    /// from Browse, a stack of three documents with cover sheets landing in the Scans folder
    /// (as a scanner delivers it; this PC has none) and split into three rows, and an UltraTax
    /// print landing in the print capture folder. Frames at 1920×1080 in `VF_WALK_DIR`. Run
    /// as a made-up login (`USERNAME=ppreparer`).
    #[test]
    #[ignore = "makes training frames"]
    fn training_scan_capture_walkthrough() {
        let Ok(out) = std::env::var("VF_WALK_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let (scans, capture, inbox) = (
            t.path().join("Scans"),
            t.path().join("Print capture"),
            t.path().join("Office inbox"),
        );
        for d in [&scans, &capture, &inbox] {
            std::fs::create_dir_all(d).unwrap();
        }
        let master = vf_fs::clients::master_dir(&root);
        let (mut fs, _) = filer_intake::config::Settings::load_or_default(&master);
        fs.inboxes = vec![inbox.clone()];
        fs.save(&master).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        h.state_mut().config.ingest.scans = Some(scans.clone());
        h.state_mut().config.ingest.print_capture = Some(capture.clone());
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            if let Some(b) = h.query_by_role_and_label(Role::Button, "Dismiss") {
                b.click();
            }
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
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
        h.run_steps(12);
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let text_of = |h: &egui_kittest::Harness<'static, VibeFilesApp>, t: &str| {
            h.query_all_by_label(t)
                .next()
                .or_else(|| h.query_all_by_label_contains(t).next())
                .unwrap_or_else(|| panic!("{t}: not shown"))
                .rect()
        };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };

        // ---- s02. Intake's scanning strip.
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_role_and_label(Role::Button, "Show scanning and inboxes") {
            b.click();
            h.run_steps(6);
        }
        let r = text_of(&h, "Scans folder").union(button(&h, "Choose…"));
        let r2 =
            button(&h, "Cover sheet for client…").union(button(&h, "Print 10 blank separators"));
        shot(&mut h, "s02a-strip", &[r, r2]);
        // Scanner details lists this PC's folders and software: not opened on camera.
        let r = text_of(&h, "No scanner found");
        shot(&mut h, "s02b-scanner", &[r]);

        // ---- s03. Print cover sheet in Browse.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        h.run_steps(10);
        // Browse hands the sheet to Windows to print: recorded here instead.
        let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::<std::path::PathBuf>::new()));
        {
            let o = opened.clone();
            open_mut(&mut h).browse.shell = Box::new(move |a| {
                if let crate::shell::ShellAction::Open(p) = a {
                    o.borrow_mut().push(p.clone());
                }
                Ok(())
            });
        }
        let r = button(&h, "Print cover sheet");
        shot(&mut h, "s03a-button", &[r]);
        h.get_by_role_and_label(Role::Button, "Print cover sheet")
            .click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::ComboBox, "Doc type").click();
        h.run_steps(6);
        if h.query_by_role_and_label(Role::Button, "Bank statement")
            .is_none()
            && h.query_by_label("Type to search").is_some()
        {
            h.get_by_label("Type to search").click();
            h.step();
            h.get_by_label("Type to search").type_text("bank");
            h.run_steps(4);
        }
        dump(&h, "doctypes");
        h.get_by_role_and_label(Role::Button, "Bank statement")
            .click();
        h.run_steps(6);
        let r = h
            .get_by_role_and_label(Role::ComboBox, "Client")
            .rect()
            .union(h.get_by_label("Sheets").rect())
            .union(button(&h, "Create cover sheet"));
        shot(&mut h, "s03b-cover", &[r]);
        h.get_by_role_and_label(Role::Button, "Create cover sheet")
            .click();
        h.run_steps(8);
        let sheet_pdf = opened.borrow().first().cloned().expect("the cover sheet");
        std::fs::copy(&sheet_pdf, out.join("cover-sheet.pdf")).unwrap();
        // Several clients.
        h.get_by_role_and_label(Role::Button, "Print cover sheet")
            .click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "Several clients")
            .click();
        h.run_steps(8);
        dump(&h, "several");
        shot(&mut h, "s03c-several", &[]);
        if h.query_by_role_and_label(Role::Button, "Cancel").is_some() {
            h.get_by_role_and_label(Role::Button, "Cancel").click();
            h.run_steps(4);
        }

        // ---- s04. A stack scanned into the Scans folder: three documents, each behind its sheet.
        let type_id = |name: &str| {
            vf_fs::doctypes::load(&root)
                .unwrap()
                .unwrap()
                .into_iter()
                .find(|t| t.name == name)
                .unwrap()
                .id
        };
        let body = |n: &str, lines: &[&str]| {
            let p = t.path().join(n);
            crate::ui::test_util::write_lines_pdf(&p, &[lines, lines]);
            p
        };
        let mut parts: Vec<(std::path::PathBuf, String)> = vec![(sheet_pdf.clone(), String::new())];
        parts.push((
            body(
                "b1.pdf",
                &[
                    "Northwind Bank NA",
                    "Statement: December 2025",
                    "Example Freight LLC",
                ],
            ),
            String::new(),
        ));
        let more: [(&str, &str, &str, &[&str]); 2] = [
            (
                "1002",
                "Placeholt, Casey",
                "W-2",
                &["Form W-2 Wage and Tax Statement 2025", "Placeholt, Casey"],
            ),
            (
                "1004",
                "Synthetic Orchards LLC",
                "1099-INT",
                &[
                    "Form 1099-INT Interest Income 2025",
                    "Synthetic Orchards LLC",
                ],
            ),
        ];
        for (k, (id, name, ty, lines)) in more.into_iter().enumerate() {
            let cover = t.path().join(format!("cover{k}.pdf"));
            vf_ingest::sheet::write_pdf(
                &cover,
                &[(
                    vf_ingest::sheet::Sheet(Some(vf_ingest::sheet::Cover {
                        client_id: id.into(),
                        doc_type: Some(type_id(ty)),
                        tax_year: Some(vf_ingest::sheet::TaxYear::Year(2025)),
                    })),
                    vf_ingest::sheet::Labels {
                        client: format!("{name} ({id})"),
                        doc_type: ty.into(),
                        tax_year: "2025".into(),
                        folder: String::new(),
                    },
                )],
            )
            .unwrap();
            parts.push((cover, String::new()));
            parts.push((body(&format!("b{}.pdf", k + 2), lines), String::new()));
        }
        let staged = t.path().join("scan0001.pdf");
        filer_pdf::pdf::merge_pdfs(&parts, &staged).unwrap();
        std::fs::copy(&staged, out.join("stack.pdf")).unwrap();
        // The scanner saves the stack into the Scans folder.
        std::fs::rename(&staged, scans.join("scan0001.pdf")).unwrap();
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        let start = std::time::Instant::now();
        while !(said(&h, "Example Freight LLC_1005_2025 Bank statement")
            && said(&h, "Placeholt, Casey_1002_2025 W-2")
            && said(&h, "Synthetic Orchards LLC_1004_2025 1099-INT"))
        {
            assert!(start.elapsed().as_secs() < 120, "the stack split: never");
            if let Some(b) = h.query_by_label("Preview") {
                b.click();
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        let r = text_of(&h, "barcode cover sheet");
        shot(&mut h, "s04a-log", &[r]);
        // The three rows in the queue.
        h.get_by_role_and_label(Role::Button, "Hide").click();
        h.run_steps(6);
        let rows = [
            "Example Freight LLC_1005_2025 Bank statement.pdf",
            "Placeholt, Casey_1002_2025 W-2.pdf",
            "Synthetic Orchards LLC_1004_2025 1099-INT.pdf",
        ];
        let start = std::time::Instant::now();
        while !rows
            .iter()
            .all(|n| h.query_by_label(&format!("Select {n}")).is_some())
        {
            assert!(start.elapsed().as_secs() < 120, "the three rows: never");
            if let Some(b) = h.query_by_label("Preview") {
                b.click();
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        h.run_steps(20);
        let r = rows
            .iter()
            .map(|n| h.get_by_label(&format!("Select {n}")).rect())
            .reduce(|a, b| a.union(b))
            .unwrap();
        let r = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 1390.0, r.max.y + 40.0));
        shot(&mut h, "s04b-split", &[r]);

        // ---- s05. Print capture: Admin › Print capture & parsers, then an UltraTax print.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(8);
        h.get_all_by_label("Print capture & parsers")
            .next()
            .unwrap()
            .scroll_to_me();
        h.run_steps(4);
        h.get_all_by_label("Print capture & parsers")
            .next()
            .unwrap()
            .click();
        h.run_steps(10);
        let r = text_of(&h, "Print capture folder").union(button(&h, "Stop capturing"));
        let r = egui::Rect::from_min_max(r.min, egui::pos2(r.max.x + 10.0, r.max.y));
        shot(&mut h, "s05a-capture-page", &[r]);
        // Try a printed name.
        h.get_by_label("Printed file name").click();
        h.step();
        h.get_by_label("Printed file name")
            .type_text("Placeholt, Morgan_2025_1040_GovernmentCopyTaxReturn_1001.pdf");
        h.get_by_role_and_label(Role::Button, "Try").click();
        h.run_steps(8);
        dump(&h, "try");
        let r = h
            .get_by_label("Printed file name")
            .rect()
            .union(button(&h, "Try"));
        shot(&mut h, "s05b-try", &[r]);
        h.get_by_role_and_label(Role::Button, "Options")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Options").union(button(&h, "Document Names"));
        shot(&mut h, "s05c-options", &[r]);
        h.get_by_role_and_label(Role::Button, "Document Names")
            .click();
        h.run_steps(6);
        let r = button(&h, "Document Names");
        shot(&mut h, "s05d-document-names", &[r]);
        // UltraTax prints a client's return to the capture folder.
        let printed = capture.join("Placeholt, Morgan_2025_1040_GovernmentCopyTaxReturn_1001.pdf");
        crate::ui::test_util::write_lines_pdf(
            &printed.with_extension("tmp"),
            crate::ui::test_util::SAMPLE_RETURN,
        );
        std::fs::rename(printed.with_extension("tmp"), &printed).unwrap();
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        let start = std::time::Instant::now();
        while !h.query_all_by_role(Role::CheckBox).any(|n| {
            use egui_kittest::kittest::NodeT;
            n.accesskit_node()
                .label()
                .is_some_and(|l| l.starts_with("Select ") && l.contains("GovernmentCopyTaxReturn"))
        }) {
            if start.elapsed().as_secs() >= 60 {
                dump(&h, "stuck");
                shot(&mut h, "zz-stuck", &[]);
                panic!("the print: never");
            }
            // Intake keeps the list still while you review: "1 new file arrived … Refresh".
            if h.query_all_by_label_contains("arrived while you were reviewing")
                .next()
                .is_some()
            {
                if !out.join("s05e-arrived.png").exists() {
                    let r = text_of(&h, "arrived while you were reviewing");
                    let r = egui::Rect::from_min_max(
                        egui::pos2(r.min.x - 100.0, r.min.y),
                        egui::pos2(r.max.x + 80.0, r.max.y),
                    );
                    shot(&mut h, "s05e-arrived", &[r]);
                }
                use egui_kittest::kittest::NodeT;
                if let Some(n) = h
                    .query_all_by_label("Refresh")
                    .find(|n| n.accesskit_node().role() != Role::Label)
                {
                    n.click();
                } else if let Some(n) = h.query_all_by_label("Refresh").next() {
                    n.click();
                }
            }
            if let Some(b) = h.query_by_label("Preview") {
                b.click();
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        }
        h.run_steps(20);
        let r = {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::CheckBox)
                .find(|n| {
                    n.accesskit_node().label().is_some_and(|l| {
                        l.starts_with("Select ") && l.contains("GovernmentCopyTaxReturn")
                    })
                })
                .unwrap()
                .rect()
        };
        let r = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 1390.0, r.max.y + 40.0));
        shot(&mut h, "s05f-printed", &[r]);
    }

    /// Owner 2026-11-27: the frames of the training video "Backing up and syncing to B2"
    /// (admins). The bucket is an in-memory stand-in for B2 (made-up endpoint, bucket and keys;
    /// the real sync code): the set-up form, the locked tab and Unlock, a push, the per-client
    /// pill in Browse, Pause, Conflicts and Restore. Frames at 1920×1080 in `VF_WALK_DIR`. Run as
    /// `USERNAME=ppreparer COMPUTERNAME=FRONTDESK-PC` with TMP in a neutral folder.
    #[test]
    #[ignore = "makes training frames"]
    fn training_sync_walkthrough() {
        let Ok(out) = std::env::var("VF_WALK_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            if let Some(b) = h.query_by_role_and_label(Role::Button, "Dismiss") {
                b.click();
            }
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
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
        h.run_steps(12);

        // ---- y02. The Sync tab before it is set up.
        h.get_by_role_and_label(Role::Button, "Sync").click();
        h.run_steps(10);
        let type_into =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, v: &str| {
                h.get_by_label(label).click();
                h.step();
                h.key_press(egui::Key::End);
                for _ in 0..60 {
                    h.key_press(egui::Key::Backspace);
                }
                h.step();
                h.get_by_label(label).type_text(v);
                h.run_steps(3);
            };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        shot(&mut h, "y02a-setup", &[]);
        // Made-up values (Connect is not pressed: the stand-in bucket is given below).
        type_into(
            &mut h,
            "S3 endpoint",
            "https://s3.us-west-004.backblazeb2.com",
        );
        type_into(&mut h, "Region", "us-west-004");
        type_into(&mut h, "Bucket", "example-firm-cabinet");
        type_into(&mut h, "Key ID", "0040000000000000000000001");
        let app_key_named = h.query_by_label("Application key").is_some()
            && h.query_all_by_label("Application key").any(|n| {
                use egui_kittest::kittest::NodeT;
                matches!(
                    n.accesskit_node().role(),
                    Role::TextInput | Role::PasswordInput
                )
            });
        eprintln!("APPKEY named: {app_key_named}");
        if app_key_named {
            h.query_all_by_label("Application key")
                .find(|n| {
                    use egui_kittest::kittest::NodeT;
                    matches!(
                        n.accesskit_node().role(),
                        Role::TextInput | Role::PasswordInput
                    )
                })
                .unwrap()
                .click();
            h.step();
            h.event(egui::Event::Text("made-up-application-key".into()));
            h.run_steps(3);
        }
        type_into(&mut h, "Sync passphrase", "made-up training passphrase");
        type_into(&mut h, "Passphrase again", "made-up training passphrase");
        dump(&h, "filled");
        let r = h
            .get_by_label("S3 endpoint")
            .rect()
            .union(h.get_by_label("Passphrase again").rect())
            .union(button(&h, "Connect"));
        shot(&mut h, "y02b-filled", &[r]);

        // ---- y03. Set up on another PC: locked until the passphrase is typed.
        let id = {
            let o = open_mut(&mut h);
            o.opened.cabinet.cabinet_id.clone()
        };
        let remote = vf_sync::Remote::new(vf_sync::Remote::memory(), &id);
        let keys = vf_sync::setup::init(
            &root,
            &remote,
            "made-up training passphrase",
            vf_sync::setup::DEFAULT_KDF,
        )
        .unwrap();
        {
            let app = h.state_mut();
            app.config.sync.endpoint = "https://s3.us-west-004.backblazeb2.com".into();
            app.config.sync.region = "us-west-004".into();
            app.config.sync.bucket = "example-firm-cabinet".into();
            app.config.sync.key_id = "0040000000000000000000001".into();
            app.config.sync.cabinet_id = id.clone();
            app.config.sync.app_key_sealed = vf_sync::setup::seal_secret(&keys, "made-up-app-key");
        }
        h.run_steps(10);
        dump(&h, "locked");
        shot(&mut h, "y03-locked", &[]);

        // ---- y04. Unlocked (the stand-in bucket), then Push now.
        {
            let o = open_mut(&mut h);
            let device = o.opened.sync_device.clone();
            o.sync.session = Some(vf_sync::Sync {
                remote,
                keys,
                cabinet_id: id.clone(),
                device_id: device,
                device_name: "FRONTDESK-PC".into(),
            });
        }
        h.run_steps(20);
        dump(&h, "unlocked");
        shot(&mut h, "y04a-unlocked", &[]);
        if h.query_by_role_and_label(Role::Button, "Push now")
            .is_some()
        {
            let r = button(&h, "Push now");
            shot(&mut h, "y04b-push", &[r]);
            h.get_by_role_and_label(Role::Button, "Push now").click();
            let start = std::time::Instant::now();
            while !said(&h, "Pushed") {
                assert!(start.elapsed().as_secs() < 180, "pushed: never");
                std::thread::sleep(std::time::Duration::from_millis(100));
                h.step();
            }
            h.run_steps(20);
            let r = h
                .query_all_by_label_contains("Pushed ")
                .next()
                .unwrap()
                .rect();
            shot(&mut h, "y04c-pushed", &[r]);
        }
        // ---- y05. What goes to B2, and the clients.
        h.get_by_label("Find a client").scroll_to_me();
        h.run_steps(6);
        let r = h.get_by_label("Find a client").rect();
        shot(&mut h, "y05-clients", &[r]);
        // ---- y06. Pause, then Restore.
        h.get_by_role_and_label(Role::Button, "Pause")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Pause");
        shot(&mut h, "y06a-pause", &[r]);
        h.get_by_role_and_label(Role::Button, "Pause").click();
        h.run_steps(8);
        dump(&h, "paused");
        shot(&mut h, "y06b-paused", &[]);
        if let Some(b) = h.query_by_role_and_label(Role::Button, "Resume") {
            b.click();
            h.run_steps(6);
        }
        // Restore opens Windows' folder picker (a person picks the folder): shown, not pressed.
        h.get_by_role_and_label(Role::Button, "Restore cabinet to a folder…")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Restore cabinet to a folder…");
        shot(&mut h, "y07-restore", &[r]);
        // ---- y08. Browse: each client's sync pill.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        h.run_steps(10);
        let start = std::time::Instant::now();
        while !said(&h, "in sync") && start.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.step();
        }
        let r = button(&h, "Sync to B2");
        shot(&mut h, "y08a-pill", &[r]);
        h.get_by_role_and_label(Role::Button, "Sync to B2").click();
        h.run_steps(8);
        dump(&h, "pillmenu");
        shot(&mut h, "y08b-pill-menu", &[]);
    }

    /// Owner 2026-11-27: the frames of the training video "Merging clients" (staff): a client
    /// made twice found by Admin › Similar clients, then merged from its panel with Merge into…. Frames at 1920×1080 in `VF_WALK_DIR`. Run as
    /// `USERNAME=ppreparer COMPUTERNAME=FRONTDESK-PC` with TMP in a neutral folder.
    #[test]
    #[ignore = "makes training frames"]
    fn training_merge_walkthrough() {
        let Ok(out) = std::env::var("VF_WALK_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        // The same client made a second time, from a folder named with a stand-in ID.
        {
            let firm: uuid::Uuid = vf_fs::cabinet::read(&root)
                .unwrap()
                .firm_id
                .parse()
                .unwrap();
            let src = root.join("Placeholt, Casey (1002)");
            let dup = root.join("Placeholt, Casey (1092)");
            std::fs::create_dir_all(dup.join("Income Tax/2025")).unwrap();
            filer_intake::sentinel::write_sentinel(
                &root,
                &dup,
                &filer_core::clients::Client {
                    id: "1092".into(),
                    uuid: uuid::Uuid::new_v4(),
                    name: "Placeholt, Casey".into(),
                    extra: Default::default(),
                },
                firm,
            )
            .unwrap();
            let mut n = 0;
            for e in std::fs::read_dir(src.join("Income Tax/2025"))
                .unwrap()
                .flatten()
            {
                if n < 3 && e.path().extension().is_some_and(|x| x == "pdf") {
                    let name = e.file_name().to_string_lossy().replace("1002", "1092");
                    std::fs::copy(e.path(), dup.join("Income Tax/2025").join(name)).unwrap();
                    n += 1;
                }
            }
        }
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            if let Some(b) = h.query_by_role_and_label(Role::Button, "Dismiss") {
                b.click();
            }
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
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
        h.run_steps(12);
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };

        // ---- m02. Admin › Similar clients finds the pair.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(10);
        h.query_all_by_label("Similar clients")
            .next()
            .unwrap()
            .click();
        h.run_steps(8);
        let r = button(&h, "Find similar clients");
        shot(&mut h, "m02a-similar", &[r]);
        h.get_by_role_and_label(Role::Button, "Find similar clients")
            .click();
        assert!(wait_for(&mut h, |h| h
            .query_all_by_label_contains("pair among")
            .next()
            .is_some()));
        h.run_steps(6);
        dump(&h, "similar");
        shot(&mut h, "m02b-pairs", &[]);

        // ---- m03. Merge into… from the duplicate's panel.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "Placeholt, Casey  1092")
            .click();
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "Merge into…")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Merge into…");
        shot(&mut h, "m03a-panel", &[r]);
        h.get_by_role_and_label(Role::Button, "Merge into…").click();
        h.run_steps(8);
        h.get_by_label("Merge into").click();
        h.step();
        h.get_by_label("Merge into").type_text("1002");
        h.run_steps(8);
        shot(&mut h, "m03b-typed", &[]);
        h.get_all_by_label("Placeholt, Casey  1002")
            .last()
            .expect("offered")
            .click();
        h.run_steps(10);
        dump(&h, "preview");
        shot(&mut h, "m03c-preview", &[]);
        h.get_by_role_and_label(Role::Button, "Merge").click();
        h.run_steps(12);
        assert!(
            h.query_by_role_and_label(Role::Button, "Placeholt, Casey  1092")
                .is_none(),
            "merged"
        );
        shot(&mut h, "m04a-merged", &[]);
        h.get_by_role_and_label(Role::Button, "Placeholt, Casey  1002")
            .click();
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "Merge into…").click();
        h.run_steps(8);
        let r = button(&h, "Undo last merge");
        shot(&mut h, "m04b-undo", &[r]);
    }

    /// Owner 2026-11-27: the frames of the training video "Admin basics" (admins): the admin
    /// password, doc types, the folder template, retention classes, disposition, legal holds
    /// and the audit trail. Frames at 1920×1080 in `VF_WALK_DIR`. Run as
    /// `USERNAME=ppreparer COMPUTERNAME=FRONTDESK-PC` with TMP in a neutral folder.
    #[test]
    #[ignore = "makes training frames"]
    fn training_admin_walkthrough() {
        let Ok(out) = std::env::var("VF_WALK_DIR") else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        crate::ui::test_util::pdfium_first();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 810.0])
            .with_pixels_per_point(1920.0 / 1440.0)
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                    name: &str,
                    marks: &[egui::Rect]| {
            if let Some(b) = h.query_by_role_and_label(Role::Button, "Dismiss") {
                b.click();
            }
            for _ in 0..6 {
                h.step();
            }
            let ppp = 1920.0 / 1440.0;
            match h.render() {
                Ok(mut img) => {
                    let (w, ht) = (img.width() as i64, img.height() as i64);
                    for r in marks {
                        let r = r.expand(5.0);
                        let (x0, y0) = ((r.min.x * ppp) as i64, (r.min.y * ppp) as i64);
                        let (x1, y1) = ((r.max.x * ppp) as i64, (r.max.y * ppp) as i64);
                        for t in 0..4 {
                            for x in x0 - t..=x1 + t {
                                for y in [y0 - t, y1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                            for y in y0 - t..=y1 + t {
                                for x in [x0 - t, x1 + t] {
                                    if (0..w).contains(&x) && (0..ht).contains(&y) {
                                        img.put_pixel(
                                            x as u32,
                                            y as u32,
                                            image::Rgba([245, 158, 11, 255]),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let dump = |h: &egui_kittest::Harness<'static, VibeFilesApp>, at: &str| {
            use egui_kittest::kittest::NodeT;
            for n in h.root().children_recursive() {
                let a = n.accesskit_node();
                let l = a.label().unwrap_or_default();
                if !l.is_empty() && !matches!(a.role(), Role::Label | Role::Unknown) {
                    eprintln!("NODE[{at}] {:?} {l:?}", a.role());
                }
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
        h.run_steps(12);
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_role_and_label(Role::Button, l).rect()
        };
        let go = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, page: &str| {
            h.query_all_by_label(page)
                .next()
                .or_else(|| h.query_all_by_label_contains(page).next())
                .unwrap()
                .scroll_to_me();
            h.run_steps(3);
            h.query_all_by_label(page)
                .next()
                .or_else(|| h.query_all_by_label_contains(page).next())
                .unwrap()
                .click();
            h.run_steps(10);
        };

        // ---- j07 first: a legal hold placed from Browse (so the Admin pages have one).
        select_in(&mut h, 0, "Income Tax/2025", "W-2");
        h.run_steps(10);
        h.get_by_role_and_label(Role::Button, "Place legal hold on this document…")
            .scroll_to_me();
        h.run_steps(4);
        let r = button(&h, "Place legal hold on this document…");
        shot(&mut h, "j07a-hold-button", &[r]);
        h.get_by_role_and_label(Role::Button, "Place legal hold on this document…")
            .click();
        h.run_steps(8);
        h.get_by_label("Hold reason").click();
        h.step();
        h.get_by_label("Hold reason")
            .type_text("IRS examination of the 2025 return");
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Place hold")
            .scroll_to_me();
        h.run_steps(4);
        let r = h
            .get_by_label("Hold reason")
            .rect()
            .union(button(&h, "Place hold"));
        shot(&mut h, "j07b-hold-dialog", &[r]);
        h.get_by_role_and_label(Role::Button, "Place hold").click();
        h.run_steps(10);
        assert!(
            h.query_by_role_and_label(Role::Button, "Place hold")
                .is_none(),
            "the hold placed"
        );

        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(10);
        // ---- j02. The admin password.
        go(&mut h, "No admin password: set one");
        h.get_by_label("New password").click();
        h.step();
        h.get_by_label("New password")
            .type_text("made-up admin password");
        h.get_by_label("New password again").click();
        h.step();
        h.get_by_label("New password again")
            .type_text("made-up admin password");
        h.run_steps(4);
        let r = h
            .get_by_label("New password")
            .rect()
            .union(button(&h, "Set admin password"));
        shot(&mut h, "j02a-password", &[r]);
        h.get_by_role_and_label(Role::Button, "Set admin password")
            .click();
        h.run_steps(10);
        let r = h
            .get_by_label("Admin password")
            .rect()
            .union(button(&h, "Unlock"));
        shot(&mut h, "j02b-locked", &[r]);
        h.get_by_label("Admin password").click();
        h.step();
        h.get_by_label("Admin password")
            .type_text("made-up admin password");
        h.get_by_role_and_label(Role::Button, "Unlock").click();
        h.run_steps(12);
        dump(&h, "unlocked");
        shot(&mut h, "j02c-unlocked", &[]);

        // ---- j03. Doc types.
        go(&mut h, "Doc types (");
        let r = button(&h, "Add doc type");
        shot(&mut h, "j03a-doctypes", &[r]);
        h.get_by_role_and_label(Role::Button, "Add doc type")
            .click();
        h.run_steps(8);
        h.get_by_label("Doc type name").click();
        h.step();
        h.get_by_label("Doc type name").type_text("Appraisal");
        h.get_by_label("Default folder").click();
        h.step();
        h.get_by_label("Default folder").type_text("Permanent");
        h.run_steps(4);
        let r = h
            .get_by_label("Doc type name")
            .rect()
            .union(h.get_by_role_and_label(Role::ComboBox, "Retention").rect())
            .union(button(&h, "Add"));
        shot(&mut h, "j03b-add", &[r]);
        h.get_by_role_and_label(Role::Button, "Add").click();
        h.run_steps(10);
        dump(&h, "added");
        shot(&mut h, "j03c-added", &[]);

        // ---- j04. The folder template.
        go(&mut h, "Folder templates");
        h.get_by_role_and_label(Role::Button, "Correspondence")
            .click();
        h.run_steps(6);
        let r = button(&h, "Add sub-folder").union(button(&h, "Add {Year} folder"));
        shot(&mut h, "j04a-template", &[r]);
        h.get_by_role_and_label(Role::Button, "Add sub-folder")
            .click();
        h.run_steps(8);
        let r = button(&h, "Save template").union(button(&h, "Discard changes"));
        shot(&mut h, "j04b-added", &[r]);
        h.get_by_role_and_label(Role::Button, "Discard changes")
            .click();
        h.run_steps(6);

        // ---- j05. Retention classes.
        go(&mut h, "Retention classes");
        let r = button(&h, "Edit Returns & workpapers");
        shot(&mut h, "j05a-retention", &[r]);
        h.get_by_role_and_label(Role::Button, "Edit Returns & workpapers")
            .click();
        h.run_steps(8);
        dump(&h, "retedit");
        shot(&mut h, "j05b-retention-edit", &[]);
        if h.query_by_role_and_label(Role::Button, "Cancel").is_some() {
            h.get_by_role_and_label(Role::Button, "Cancel").click();
            h.run_steps(6);
        }

        // ---- j06. Disposition; j07. Legal holds.
        go(&mut h, "Disposition");
        shot(&mut h, "j06-disposition", &[]);
        go(&mut h, "Legal holds");
        dump(&h, "holds");
        shot(&mut h, "j07c-holds", &[]);

        // ---- j08. The audit trail.
        go(&mut h, "Audit trail");
        let r = button(&h, "Verify now");
        let r2 = button(&h, "Export CSV…").union(button(&h, "Export PDF…"));
        shot(&mut h, "j08a-audit", &[r, r2]);
        h.get_by_role_and_label(Role::Button, "Verify now").click();
        h.run_steps(10);
        shot(&mut h, "j08b-verified", &[]);
    }

    /// Addendum §12 slice 3 (letters) walkthrough against a real Vibe Portal: the Deliver window
    /// sends the engagement letter for a joint 1040 with both spouses as signers (from Portal
    /// contacts); in headless Edge the taxpayer signs, then the spouse on the same device after
    /// a code; the step 1 PDF (the signed letter, then its log) is committed by Intake into
    /// `Income Tax\2025\Delivery`. Same variables as `delivery_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn letters_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let folder = c.folder_rel.clone().unwrap();
        // The couple's Portal contacts (PA-105), as typed in the client panel.
        vf_fs::contacts::set_by_hand(
            &root,
            &c.id,
            &vf_fs::contacts::Row {
                first: "Jordan".into(),
                last: "Sample".into(),
                email: "jordan.sample@example.com".into(),
                spouse_first: "Casey".into(),
                spouse_last: "Sample".into(),
                spouse_email: "casey.sample@example.com".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Taxpayer",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}) on the portal; Jordan invited; Jordan and Casey in Portal contacts",
            c.name, c.ext_id
        ));

        // ---- Step 1: Deliver… with the engagement letter, both spouses signing.
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| said(h, "WHAT HAS HAPPENED"));
        h.run_steps(6);
        for label in ["Tax return PDF", "Form 8879", "Letters to sign"] {
            h.get_by_label(label).click();
            h.run_steps(3);
        }
        wait_long(&mut h, "the signers", &|h| said(h, "Who signs"));
        shot(&mut h, "01-deliver-letters");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        let sent = p.deliveries(Some(&c.id)).unwrap();
        let did = sent[0].id.clone();
        let full = p.delivery(&did).unwrap();
        assert_eq!(
            full.letters.len(),
            1,
            "the engagement letter (the consents start unticked)"
        );
        assert_eq!(
            full.signers
                .iter()
                .map(|s| s.role.as_str())
                .collect::<Vec<_>>(),
            ["taxpayer", "spouse"]
        );
        note(format!(
            "Step 1: sent with \"{}\" ({} page(s)), signers {:?}",
            full.letters[0].name,
            full.letters[0].pages,
            full.signers
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
        ));

        // ---- Step 2: Jordan signs, then Casey on the same device after a code.
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                "jordan.sample@example.com",
                "letters",
                "casey.sample@example.com",
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client's signing (see client-letters.json)");
        let now = p.delivery(&did).unwrap();
        assert_eq!(
            now.step(vf_portal::delivery::LETTERS)
                .map(|s| s.state.as_str()),
            Some("done")
        );
        let vias: Vec<String> = now.letters[0]
            .signatures
            .iter()
            .map(|g| g.via.clone())
            .collect();
        assert_eq!(vias.len(), 2);
        assert!(
            vias.contains(&"own_login".to_string()) && vias.contains(&"same_device".to_string())
        );
        assert!(now.letters[0].signed_sha256.is_some());
        note("Step 2: Jordan signed (own login), Casey signed after a code (same device); step 1 done".into());
        {
            let o = open_mut(&mut h);
            o.browse.portal.delivery = None;
        }

        // ---- Step 3: the step 1 PDF is fetched and filed.
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let found = |inbox: &std::path::Path| {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .contains("2025 Delivery 1 - Letters")
                })
        };
        wait_long(&mut h, "the letters PDF fetched", &|_| found(&inbox));
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        wait_long(&mut h, "the row ready", &|h| said(h, "Commit 1 file"));
        shot(&mut h, "02-intake-letters-pdf");
        h.query_all_by_role_and_label(Role::Button, "Commit 1 file")
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        let filed = root
            .join(&folder)
            .join("Income Tax")
            .join("2025")
            .join("Delivery")
            .join("2025 Delivery 1 - Letters.pdf");
        wait_long(&mut h, "filed", &|_| filed.exists());
        let bytes = std::fs::read(&filed).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        note(format!(
            "Step 3: committed {folder}\\Income Tax\\2025\\Delivery\\2025 Delivery 1 - Letters.pdf ({} bytes: the signed letter, then the log page)",
            bytes.len()
        ));
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Addendum §12 slice 4 (packages and the fee) walkthrough against a real Vibe Portal and the
    /// stand-in Stripe: the sample plan priced for a $1,265.68 return fee with Concierge
    /// overridden, sent with the addenda; the client chooses Advantage and an add-on, reads the
    /// addendum, signs, pays by card; steps 3 and 4's PDFs are committed into
    /// `Income Tax\2025\Delivery`; the sent prices stand when the plan is edited. Set
    /// `VF_WALK_URL`, `VF_WALK_KEY`, `VF_WALK_LOG`, `VF_WALK_CLIENT`, `VF_WALK_STRIPE` and
    /// `VF_WALK_DIR`.
    #[test]
    #[ignore = "needs a running portal"]
    fn fee_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        vf_portal::pricing::save(&root, &vf_portal::pricing::sample("1040", 2025)).unwrap();
        vf_letters::store::seed_if_missing(&root).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let folder = c.folder_rel.clone().unwrap();
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Taxpayer",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}) on the portal; the sample 2025 1040 plan; the addendum samples",
            c.name, c.ext_id
        ));

        // ---- Step 1: Deliver… with packages priced for the client.
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| said(h, "WHAT HAS HAPPENED"));
        h.run_steps(6);
        for label in ["Tax return PDF", "Form 8879", "Package and fee"] {
            h.get_by_label(label).click();
            h.run_steps(3);
        }
        h.get_by_label("Return fee").click();
        h.step();
        h.get_by_label("Return fee").type_text("1265.68");
        h.run_steps(3);
        // An override for this client: Concierge at $1,700.00.
        h.get_by_label("Concierge price").click();
        h.step();
        // The worked-out price is erased from its end (the box has focus), then the override
        // typed.
        h.key_press(egui::Key::End);
        h.step();
        for _ in 0..12 {
            h.key_press(egui::Key::Backspace);
            h.step();
        }
        h.get_by_label("Concierge price").type_text("1700.00");
        h.run_steps(4);
        shot(&mut h, "01-deliver-fee");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        let did = p.deliveries(Some(&c.id)).unwrap()[0].id.clone();
        let sent = p.delivery(&did).unwrap();
        let fee = sent.fee.clone().unwrap();
        let price = |name: &str| fee.lines.iter().find(|l| l.name == name).map(|l| l.cents);
        assert_eq!(price("Tax Return Only"), Some(126_568));
        assert_eq!(price("Advantage"), Some(139_268));
        assert_eq!(price("Concierge"), Some(170_000), "the override is kept");
        note("Step 1: sent with Tax Return Only $1,265.68, Advantage $1,392.68 (from the plan), Concierge $1,700.00 (overridden), two add-ons, and the addenda".into());

        // ---- Step 2: the client chooses, signs and pays.
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                "jordan.sample@example.com",
                "fee",
                &stripe,
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client's choice and payment (see client-fee.json)");
        let now = p.delivery(&did).unwrap();
        let f = now.fee.clone().unwrap();
        assert_eq!(f.total_cents, 139_268 + 25_000);
        assert!(f.selection_signed_at.is_some());
        assert_eq!(f.payment_words(), "Paid $1,642.68 by card");
        note(format!(
            "Step 2: Advantage and the planning meeting signed; {}",
            f.payment_words()
        ));

        // ---- Step 3: a plan edit leaves the sent delivery's prices as they were.
        let mut plan = vf_portal::pricing::load(&root, "1040", 2025).unwrap();
        plan.packages[1].minimum_cents = 50_000;
        vf_portal::pricing::save(&root, &plan).unwrap();
        let again = p.delivery(&did).unwrap().fee.unwrap();
        assert_eq!(again.lines, f.lines);
        note(
            "Step 3: the plan's Advantage minimum raised to $500; the delivery's prices stand"
                .into(),
        );

        // ---- Step 4: the step 3 and 4 PDFs are fetched and filed.
        open_mut(&mut h).browse.portal.delivery = None;
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let count = |inbox: &std::path::Path| {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains(" Delivery "))
                .count()
        };
        wait_long(&mut h, "both PDFs fetched", &|_| count(&inbox) == 2);
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        wait_long(&mut h, "both rows ready", &|h| said(h, "Commit 2 files"));
        shot(&mut h, "02-intake-fee-pdfs");
        h.query_all_by_role_and_label(Role::Button, "Commit 2 files")
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        let dir = root
            .join(&folder)
            .join("Income Tax")
            .join("2025")
            .join("Delivery");
        for want in [
            "2025 Delivery 3 - Package selection.pdf",
            "2025 Delivery 4 - Fee.pdf",
        ] {
            let path = dir.join(want);
            wait_long(&mut h, want, &|_| path.exists());
        }
        note(format!(
            "Step 4: committed into {folder}\\Income Tax\\2025\\Delivery: the package selection (with the signed Advantage addendum) and the fee receipt"
        ));
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Owner 2026-11-26 walkthrough against a real Vibe Portal and the stand-in Stripe: a tax
    /// return delivered with a fee; the client views the return and pays; a change is found, so
    /// a corrected return is sent as revised on the same card (the fee stays paid, the client
    /// sees Updated and opens it); Form 8879 is signed on paper; the finished card shows
    /// read-only, Send another starts with Fee unticked, and Revise the return… opens the
    /// finished card again with its fee paid. Same variables as `fee_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn revise_walkthrough() {
        use egui_kittest::kittest::NodeT;
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        const FIRST: &str = "2025 Form 1040 Return - Sample.pdf";
        const FIXED: &str = "2025 Form 1040 Return - Sample corrected.pdf";
        const AGAIN: &str = "2025 Form 1040 Return - Sample second correction.pdf";
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') {
                let dir = e.path().join("Income Tax").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join(FIRST),
                    crate::ui::test_util::SAMPLE_RETURN,
                );
                crate::ui::test_util::write_lines_pdf(
                    &dir.join(FIXED),
                    &[&["Sample client", "Form 1040 (corrected)", "2025"]],
                );
                crate::ui::test_util::write_lines_pdf(
                    &dir.join(AGAIN),
                    &[&["Sample client", "Form 1040 (second correction)", "2025"]],
                );
            }
        }
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::DELIVERY_FOLDER, true)
                .unwrap();
        }
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
                }
            };
        let pick = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, now: &str, pick: &str| {
            let before: Vec<_> = h
                .query_all_by_label_contains(pick)
                .map(|n| n.accesskit_node().id())
                .collect();
            h.query_all_by_role(Role::ComboBox)
                .rfind(|n| {
                    let a = n.accesskit_node();
                    a.value().as_deref() == Some(now) || a.label().as_deref() == Some(now)
                })
                .unwrap_or_else(|| panic!("no combo showing {now:?}"))
                .click();
            h.run_steps(4);
            h.query_all_by_label_contains(pick)
                .rfind(|n| !before.contains(&n.accesskit_node().id()))
                .unwrap_or_else(|| panic!("no choice {pick:?}"))
                .click();
            h.run_steps(4);
        };
        let ticked = |h: &egui_kittest::Harness<'static, VibeFilesApp>, label: &str| {
            h.query_all_by_label(label)
                .find(|n| n.accesskit_node().role() == Role::CheckBox)
                .map(|n| n.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True))
        };
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, label: &str| {
            h.query_all_by_label(label)
                .any(|n| n.accesskit_node().role() == Role::Button)
        };
        let client = |phase: &str| {
            std::process::Command::new("node")
                .args([
                    &client_js,
                    &url,
                    &out.display().to_string(),
                    &log,
                    "jordan.sample@example.com",
                    phase,
                    &stripe,
                ])
                .status()
                .is_ok_and(|s| s.success())
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Owner",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}) on the portal, Jordan invited; {FIRST}, {FIXED} and {AGAIN} in Income Tax\\2025",
            c.name, c.ext_id
        ));
        let open_window = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>| {
            open_mut(h).browse.portal.delivery = None;
            h.run_steps(4);
            h.get_by_role_and_label(Role::Button, "Deliver…").click();
            let start = std::time::Instant::now();
            while h
                .query_all_by_label_contains("WHAT HAS HAPPENED")
                .next()
                .is_none()
            {
                assert!(start.elapsed().as_secs() < 90, "the window read: never");
                std::thread::sleep(std::time::Duration::from_millis(50));
                h.step();
            }
            h.run_steps(6);
        };

        // ---- Step 1: Deliver… the return, Form 8879 and a $500 fee (one amount).
        open_window(&mut h);
        if ticked(&h, "Tax return PDF") != Some(true) {
            h.get_by_label("Tax return PDF").click();
            h.run_steps(4);
        }
        pick(&mut h, "The first PDF", FIRST);
        h.get_by_label("eSignature link address").click();
        h.step();
        h.get_by_label("eSignature link address")
            .type_text("https://esign.example-tax.test/s/revise");
        h.run_steps(3);
        if ticked(&h, "Package and fee") != Some(true) {
            h.get_by_label("Package and fee").click();
            h.run_steps(4);
        }
        h.get_by_label("One amount").click();
        h.run_steps(3);
        h.get_by_label("Delivery fee").click();
        h.step();
        h.get_by_label("Delivery fee").type_text("500");
        h.run_steps(3);
        shot(&mut h, "01-revise-deliver");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| {
            said(h, "Sent to the client.") || said(h, "rror")
        });
        assert!(said(&h, "Sent to the client."), "see 01-revise-deliver.png");
        let did = p.deliveries(Some(&c.id)).unwrap()[0].id.clone();
        let d0 = p.delivery(&did).unwrap();
        note(format!(
            "Step 1: sent \"{}\" with {:?}, fee {}; {}",
            d0.title,
            d0.return_file.as_ref().map(|f| f.name.clone()),
            d0.fee.as_ref().map(|f| f.total_cents).unwrap_or(0),
            d0.line()
        ));

        // ---- Step 2: the client views the return and pays.
        assert!(client("revise-pay"), "see client-revise-pay.json");
        let paid = p.delivery(&did).unwrap();
        let fee_words = paid
            .fee
            .as_ref()
            .map(|f| f.payment_words())
            .unwrap_or_default();
        note(format!(
            "Step 2: the client viewed and paid: {}; {fee_words}",
            paid.line()
        ));
        assert_eq!(
            paid.step(vf_portal::delivery::FEE)
                .map(|s| s.state.as_str()),
            Some("done")
        );

        // ---- Step 3: a change is found: the corrected return, sent as revised on this card.
        open_window(&mut h);
        pick(&mut h, "The first PDF", FIXED);
        h.get_by_role_and_label(Role::Button, "Save changes")
            .click();
        wait_long(&mut h, "the revise question", &|h| {
            said(h, "Send the revised return") || said(h, "rror")
        });
        // The window scrolls the question into view the frame it appears.
        h.run_steps(4);
        shot(&mut h, "02-revise-ask");
        h.get_by_role_and_label(Role::Button, "Send the revised return")
            .click();
        wait_long(&mut h, "saved", &|h| {
            said(h, "Saved.") || said(h, "rror") || said(h, "said no")
        });
        shot(&mut h, "03-revise-saved");
        assert!(said(&h, "Saved."), "see 03-revise-saved.png");
        let rev = p.delivery(&did).unwrap();
        let state = |d: &vf_portal::delivery::Delivery, k: &str| {
            d.step(k).map(|s| s.state.clone()).unwrap_or_default()
        };
        let payments = rev.fee.as_ref().map(|f| f.payments.len()).unwrap_or(0);
        note(format!(
            "Step 3: revised: return {:?}; steps return {}, fee {}, 8879 {}; payments {payments}; {}",
            rev.return_file.as_ref().map(|f| f.name.clone()),
            state(&rev, vf_portal::delivery::RETURN),
            state(&rev, vf_portal::delivery::FEE),
            state(&rev, vf_portal::delivery::ESIGN),
            rev.fee
                .as_ref()
                .map(|f| f.payment_words())
                .unwrap_or_default()
        ));
        assert_eq!(
            rev.return_file.as_ref().map(|f| f.name.as_str()),
            Some(FIXED)
        );
        assert_eq!(state(&rev, vf_portal::delivery::RETURN), "open");
        assert_eq!(state(&rev, vf_portal::delivery::FEE), "done");
        assert_eq!(payments, 1);

        // ---- Step 4: the client sees Updated, no new bill, and opens the corrected return.
        assert!(client("revise-view"), "see client-revise-view.json");
        note("Step 4: the client saw Updated with the fee still paid and opened the corrected return (client-revise-view.json)".into());

        // ---- Step 5: Form 8879 signed on paper.
        open_window(&mut h);
        h.get_by_role_and_label(Role::Button, "Signed on paper…")
            .click();
        h.run_steps(4);
        h.get_by_label("Signed on paper note").click();
        h.step();
        h.get_by_label("Signed on paper note")
            .type_text("Signed at the office");
        h.run_steps(3);
        shot(&mut h, "04-paper-ask");
        h.get_by_role_and_label(Role::Button, "Mark signed").click();
        wait_long(&mut h, "marked", &|h| {
            said(h, "Form 8879 marked signed on paper.") || said(h, "rror") || said(h, "said no")
        });
        shot(&mut h, "05-paper-marked");
        let done = p.delivery(&did).unwrap();
        let words: Vec<String> = done.events.iter().map(|e| e.words.clone()).collect();
        note(format!(
            "Step 5: Signed on paper: status {}; the log says {:?}",
            done.status,
            words.iter().find(|w| w.contains("paper"))
        ));
        assert_eq!(done.status, "complete");
        assert!(
            words
                .iter()
                .any(|w| w == "Signed on paper (marked by the firm): Signed at the office")
        );

        // ---- Step 6: the finished card read-only; Send another starts with Fee unticked.
        open_window(&mut h);
        shot(&mut h, "06-finished");
        let shown = (
            said(&h, "Nothing here can be changed."),
            button(&h, "Revise the return…"),
            button(&h, "Send another"),
            button(&h, "Save changes"),
        );
        note(format!(
            "Step 6: finished: read-only, Revise the return…, Send another, Save changes: {shown:?}"
        ));
        assert_eq!(shown, (true, true, true, false), "see 06-finished.png");
        h.get_by_role_and_label(Role::Button, "Send another")
            .click();
        h.run_steps(6);
        shot(&mut h, "07-send-another");
        let fresh = (
            ticked(&h, "Package and fee"),
            said(&h, "card's fee was paid. Tick Fee to bill again."),
        );
        note(format!(
            "Step 6: Send another: Package and fee ticked, the note: {fresh:?}"
        ));
        assert_eq!(fresh, (Some(false), true), "see 07-send-another.png");

        // ---- Step 7: Revise the return… on the finished card: open again, fee still paid.
        open_window(&mut h);
        h.get_by_role_and_label(Role::Button, "Revise the return…")
            .click();
        h.run_steps(4);
        pick(&mut h, "The first PDF", AGAIN);
        h.get_by_role_and_label(Role::Button, "Save changes")
            .click();
        wait_long(&mut h, "the revise question", &|h| {
            said(h, "Send the revised return") || said(h, "rror")
        });
        // The window scrolls the question into view the frame it appears.
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Send the revised return")
            .click();
        wait_long(&mut h, "saved", &|h| {
            said(h, "Saved.") || said(h, "rror") || said(h, "said no")
        });
        shot(&mut h, "08-finished-revised");
        assert!(said(&h, "Saved."), "see 08-finished-revised.png");
        let again = p.delivery(&did).unwrap();
        note(format!(
            "Step 7: the finished card revised: status {}; return {:?}; steps return {}, fee {}, 8879 {}; payments {}",
            again.status,
            again.return_file.as_ref().map(|f| f.name.clone()),
            state(&again, vf_portal::delivery::RETURN),
            state(&again, vf_portal::delivery::FEE),
            state(&again, vf_portal::delivery::ESIGN),
            again.fee.as_ref().map(|f| f.payments.len()).unwrap_or(0)
        ));
        assert_eq!(again.status, "open");
        assert_eq!(
            again.return_file.as_ref().map(|f| f.name.as_str()),
            Some(AGAIN)
        );
        assert_eq!(state(&again, vf_portal::delivery::FEE), "done");
        assert_eq!(state(&again, vf_portal::delivery::RETURN), "open");
        assert_eq!(again.fee.as_ref().map(|f| f.payments.len()), Some(1));

        // ---- Step 8 (owner 2026-11-26): a 2024 card whose Form 8879 is signed at the office:
        // no link, the step open at once.
        open_window(&mut h);
        // The year as the Deliveries page sets it (typing in the year box is the window's own).
        open_mut(&mut h)
            .browse
            .portal
            .delivery
            .as_mut()
            .unwrap()
            .focus("return", 2024, "1040");
        h.run_steps(6);
        for label in ["Tax return PDF", "Package and fee"] {
            if ticked(&h, label) == Some(true) {
                h.get_by_label(label).click();
                h.run_steps(3);
            }
        }
        if ticked(&h, "Form 8879") != Some(true) {
            h.get_by_label("Form 8879").click();
            h.run_steps(3);
        }
        h.get_by_label("Pick up and sign at our office").click();
        h.run_steps(4);
        shot(&mut h, "09-office-8879");
        let link_box = h.query_by_label("eSignature link address").is_some();
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| {
            said(h, "Sent to the client.") || said(h, "rror") || said(h, "said no")
        });
        shot(&mut h, "10-office-sent");
        assert!(said(&h, "Sent to the client."), "see 10-office-sent.png");
        let all = p.deliveries(Some(&c.id)).unwrap();
        let office = all.iter().find(|d| d.tax_year == 2024).unwrap().clone();
        note(format!(
            "Step 8: a 2024 card, Form 8879 at the office: link box shown {link_box}; esign_mode {:?}; {}",
            office.esign_mode,
            office.line()
        ));
        assert_eq!(office.esign_mode.as_deref(), Some("office"));
        assert_eq!(state(&office, vf_portal::delivery::ESIGN), "open");
        assert!(!link_box);

        // ---- Step 9: a 2023 card by mail (made here as the window would); the client sees both
        // cards' words and says the mailed one is mailed back.
        let mail = p
            .create_delivery(&vf_portal::delivery::NewDelivery {
                client_id: c.id.clone(),
                tax_year: 2023,
                return_type: "1040".into(),
                title: "Your 2023 tax return".into(),
                steps: vf_portal::delivery::StepsIn {
                    esign8879: true,
                    ..Default::default()
                },
                locks: vf_portal::delivery::Locks::default(),
                assigned_staff_id: None,
                notify: false,
                hold: false,
                remind_days: 0,
                kind: None,
                covers: Vec::new(),
                return_year: None,
                esign_mode: Some("mail".into()),
            })
            .unwrap();
        assert!(client("office-mail"), "see client-office-mail.json");
        let mailed = p.delivery(&mail.id).unwrap();
        note(format!(
            "Step 9: the client saw the office and mail words and said the 2023 one is mailed back: {}",
            mailed.line()
        ));
        assert_eq!(state(&mailed, vf_portal::delivery::ESIGN), "waiting");

        // ---- Step 10: Admin › Portal › Deliveries: Tick all shown, Mark ticked signed on paper.
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Deliveries").click();
        wait_long(&mut h, "the cards listed", &|h| said(h, "at the office:"));
        h.run_steps(6);
        shot(&mut h, "11-deliveries-office-mail");
        h.get_by_role_and_label(Role::Button, "Tick all shown")
            .click();
        h.run_steps(4);
        h.query_all_by_role(Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Mark ticked signed on paper"))
            })
            .expect("the Mark ticked button")
            .click();
        h.run_steps(4);
        h.get_by_label("Signed on paper note").click();
        h.step();
        h.get_by_label("Signed on paper note")
            .type_text("Signed paper copies received");
        h.run_steps(3);
        shot(&mut h, "12-mark-ticked-ask");
        h.query_all_by_role(Role::Button)
            .find(|n| {
                n.accesskit_node()
                    .label()
                    .is_some_and(|l| l.starts_with("Mark ") && l.ends_with(" signed"))
            })
            .expect("the Mark n signed button")
            .click();
        wait_long(&mut h, "marked", &|h| said(h, "marked signed on paper."));
        h.run_steps(6);
        shot(&mut h, "13-mark-ticked-done");
        let words: Vec<String> = h
            .query_all_by_label_contains("marked signed on paper.")
            .filter_map(|n| n.accesskit_node().label())
            .collect();
        let after = p.deliveries(Some(&c.id)).unwrap();
        let status = |y: i32| {
            after
                .iter()
                .find(|d| d.tax_year == y)
                .map(|d| d.status.clone())
                .unwrap_or_default()
        };
        note(format!(
            "Step 10: Mark ticked signed on paper: the page says {words:?}; 2025 {}, 2024 {}, 2023 {}",
            status(2025),
            status(2024),
            status(2023)
        ));
        assert_eq!(
            (status(2025), status(2024), status(2023)),
            ("open".into(), "complete".into(), "complete".into())
        );
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// PA-114 (ADDENDUM-deliver-more.md) walkthrough against a real Vibe Portal and the stand-in
    /// Stripe: Deliver… with Kind Financial statements, two PDFs and a fee the documents wait
    /// for; the client sees their names locked; Remind ticked on Admin › Portal › Deliveries
    /// reaches them; they pay and open each (the step done only after both), Read again says
    /// so, and the step PDFs are committed into `Financials\2025\Delivery`; then Balance owed…
    /// with Hold the client's files: the client's files are held until they pay. Same variables
    /// as `fee_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn deliver_more_walkthrough() {
        use egui_kittest::kittest::NodeT;
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        const SHEET: &str = "2025 Balance Sheet - Sample.pdf";
        const INCOME: &str = "2025 Income Statement - Sample.pdf";
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') {
                let dir = e.path().join("Financials").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join(SHEET),
                    &[&["Sample client", "Balance sheet", "December 31, 2025"]],
                );
                crate::ui::test_util::write_lines_pdf(
                    &dir.join(INCOME),
                    &[&[
                        "Sample client",
                        "Income statement",
                        "Year ended December 31, 2025",
                    ]],
                );
            }
        }
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true)
            .with_balances(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        // What Admin › Portal's Save does when deliveries and balances are ticked.
        for (name, folder) in vf_portal::delivery::all_step_doc_types() {
            vf_fs::doctypes::ensure(&root, name, folder, true).unwrap();
        }
        vf_fs::doctypes::ensure(
            &root,
            vf_portal::inbox::RECEIPT_DOC_TYPE,
            "Billing/{Year}",
            true,
        )
        .unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 1000.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    if start.elapsed().as_secs() > 90 {
                        let _ = h.render().map(|img| {
                            img.save(out.join(format!(
                                "FAILED-{}.png",
                                what.replace(|c: char| !c.is_alphanumeric(), "-")
                            )))
                        });
                        panic!("{what}: never");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
                }
            };
        // Opens the combo box named or showing `now` (the last such) and picks `pick`.
        let pick = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, now: &str, pick: &str| {
            let before: Vec<_> = h
                .query_all_by_label_contains(pick)
                .map(|n| n.accesskit_node().id())
                .collect();
            h.query_all_by_role(Role::ComboBox)
                .rfind(|n| {
                    let a = n.accesskit_node();
                    a.value().as_deref() == Some(now) || a.label().as_deref() == Some(now)
                })
                .unwrap_or_else(|| panic!("no combo showing {now:?}"))
                .click();
            h.run_steps(4);
            h.query_all_by_label_contains(pick)
                .rfind(|n| !before.contains(&n.accesskit_node().id()))
                .unwrap_or_else(|| panic!("no choice {pick:?}"))
                .click();
            h.run_steps(4);
        };
        let client = |phase: &str| {
            std::process::Command::new("node")
                .args([
                    &client_js,
                    &url,
                    &out.display().to_string(),
                    &log,
                    "jordan.sample@example.com",
                    phase,
                    &stripe,
                ])
                .status()
                .is_ok_and(|s| s.success())
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let folder = c.folder_rel.clone().unwrap();
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Owner",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}) on the portal, Jordan invited; {SHEET} and {INCOME} in Financials\\2025",
            c.name, c.ext_id
        ));

        // ---- Step 1: Deliver…, Kind Financial statements, two PDFs, a fee paid first.
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| said(h, "WHAT HAS HAPPENED"));
        h.run_steps(6);
        shot(&mut h, "00-deliver-opened");
        pick(&mut h, "Kind", "Financial statements");
        h.run_steps(6);
        shot(&mut h, "01-kind-statements");
        let ticked = |h: &egui_kittest::Harness<'static, VibeFilesApp>, label: &str| {
            h.query_all_by_label(label)
                .find(|n| n.accesskit_node().role() == Role::CheckBox)
                .map(|n| n.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True))
        };
        note(format!(
            "Step 1: Kind Financial statements; title box reads the default; ticks: PDF {:?}, Fee {:?}, letters {:?}",
            ticked(&h, "Financial statements PDF"),
            ticked(&h, "Fee"),
            ticked(&h, "Letters to sign")
        ));
        if ticked(&h, "Financial statements PDF") != Some(true) {
            h.get_by_label("Financial statements PDF").click();
            h.run_steps(4);
        }
        // The first PDF: whatever the window offered, the balance sheet.
        let first_now = h
            .query_all_by_role(Role::ComboBox)
            .find(|n| n.accesskit_node().label().as_deref() == Some("The first PDF"))
            .and_then(|n| n.accesskit_node().value())
            .unwrap_or_default();
        note(format!("Step 1: the first PDF offered: {first_now:?}"));
        pick(&mut h, "The first PDF", SHEET);
        h.get_by_role_and_label(Role::Button, "+ Add another PDF")
            .click();
        h.run_steps(4);
        pick(&mut h, "Another PDF", INCOME);
        if ticked(&h, "Fee") != Some(true) {
            h.get_by_label("Fee").click();
            h.run_steps(4);
        }
        h.get_by_label("Delivery fee").click();
        h.step();
        h.get_by_label("Delivery fee").type_text("450");
        h.run_steps(3);
        h.get_by_label(crate::ui::delivery::PAY_FIRST_WORDS).click();
        h.run_steps(4);
        shot(&mut h, "02-deliver-statements-filled");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| {
            said(h, "Sent to the client.") || said(h, "rror")
        });
        shot(&mut h, "03-deliver-statements-sent");
        assert!(
            said(&h, "Sent to the client."),
            "see 03-deliver-statements-sent.png"
        );
        let sent = p.deliveries_all(Some(&c.id)).unwrap();
        assert_eq!(sent.len(), 1);
        let did = sent[0].id.clone();
        let d0 = p.delivery(&did).unwrap();
        assert_eq!(d0.kind, "statements");
        assert_eq!(d0.return_type, "Financial statements");
        let names: Vec<&str> = d0.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [SHEET, INCOME]);
        assert_eq!(d0.locks.return_after_fee, Some(true));
        note(format!(
            "Step 1: sent \"{}\": kind {}, documents {names:?}, fee {}, documents after the fee: {:?}; {}",
            d0.title,
            d0.kind,
            d0.fee.as_ref().map(|f| f.total_cents).unwrap_or(0),
            d0.locks.return_after_fee,
            d0.line()
        ));

        // ---- Step 2: the client accepts and sees the documents' names, locked.
        assert!(client("statements"), "see client-statements.json");
        note(
            "Step 2: the client signed in; see client-statements.json for what the card showed"
                .into(),
        );
        // The client panel's line, while the card is open.
        open_mut(&mut h).browse.portal.look_at_deliveries_now();
        wait_long(&mut h, "the panel line", &|h| {
            said(h, "2025 Financial statements: 0 of 2 done")
        });
        shot(&mut h, "03b-panel-line");
        note(
            "Step 2: the client panel shows the card: 2025 Financial statements: 0 of 2 done"
                .into(),
        );

        // ---- Step 3: Remind ticked on Admin › Portal › Deliveries.
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(4);
        let mails_before = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"msg\":\"notify (") && l.contains("jordan.sample@example.com"))
            .count();
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Deliveries").click();
        wait_long(&mut h, "the card listed", &|h| said(h, &c.name));
        h.run_steps(6);
        shot(&mut h, "04-deliveries-page");
        h.query_all_by_label("Tick row")
            .next()
            .expect("a row to tick")
            .click();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Remind ticked")
            .click();
        h.run_steps(4);
        shot(&mut h, "05-remind-confirm");
        h.get_by_role_and_label(Role::Button, "Remind").click();
        wait_long(&mut h, "the run's words", &|h| {
            said(h, "reminded")
                || said(h, "Reminded")
                || said(h, "nobody")
                || said(h, "Nobody")
                || said(h, "failed")
                || said(h, "too often")
        });
        h.run_steps(6);
        shot(&mut h, "06-remind-done");
        let words: Vec<String> = h
            .query_all_by_label_contains("emind")
            .filter_map(|n| n.accesskit_node().label())
            .collect();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let mails_after = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"msg\":\"notify (") && l.contains("jordan.sample@example.com"))
            .count();
        note(format!(
            "Step 3: Remind ticked, 1 row: the page says {words:?}; messages to Jordan in the log: {mails_before} before, {mails_after} after"
        ));
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);

        // ---- Step 4: the client pays and opens each document.
        assert!(client("statements-pay"), "see client-statements-pay.json");
        let now = p.delivery(&did).unwrap();
        let opened: Vec<bool> = now.files.iter().map(|f| f.opened_at.is_some()).collect();
        note(format!(
            "Step 4: paid; documents opened {opened:?}; steps {:?}; status {}",
            now.steps
                .iter()
                .map(|s| format!("{} {}", s.kind, s.state))
                .collect::<Vec<_>>(),
            now.status
        ));
        assert_eq!(opened, [true, true]);
        assert_eq!(now.status, "complete");

        // ---- Step 5 (owner 2026-11-26): the finished card stays on the panel and opens in
        // Deliver… read-only, with Send another for a new card.
        open_mut(&mut h).browse.portal.look_at_deliveries_now();
        wait_long(&mut h, "the panel line, complete", &|h| {
            said(h, "2025 Financial statements: complete")
        });
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the window read", &|h| said(h, "WHAT HAS HAPPENED"));
        h.run_steps(6);
        pick(&mut h, "Kind", "Financial statements");
        h.run_steps(6);
        shot(&mut h, "06a-deliver-finished");
        let button = |h: &egui_kittest::Harness<'static, VibeFilesApp>, label: &str| {
            h.query_all_by_label(label)
                .any(|n| n.accesskit_node().role() == Role::Button)
        };
        let shown = (
            said(&h, "Nothing here can be changed."),
            button(&h, "Send another"),
            button(&h, "Save changes"),
            button(&h, "Send to client"),
        );
        note(format!(
            "Step 5: Deliver… on the finished card: read-only words, Send another, Save changes, Send to client: {shown:?}"
        ));
        assert_eq!(
            shown,
            (true, true, false, false),
            "see 06a-deliver-finished.png"
        );
        h.get_by_role_and_label(Role::Button, "Send another")
            .click();
        h.run_steps(6);
        shot(&mut h, "06b-send-another");
        let fresh = (said(&h, "Not sent yet"), button(&h, "Send to client"));
        note(format!(
            "Step 5: Send another: Not sent yet, Send to client: {fresh:?}"
        ));
        assert_eq!(fresh, (true, true), "see 06b-send-another.png");
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(4);

        // ---- Step 6: the step PDFs are fetched and committed into Financials\2025\Delivery.
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let names = |inbox: &std::path::Path| -> Vec<String> {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(" Statements "))
                .collect()
        };
        wait_long(&mut h, "the step PDFs fetched", &|_| {
            names(&inbox).len() >= 2
        });
        h.run_steps(20);
        let fetched = names(&inbox);
        note(format!("Step 6: in the Portal inbox: {fetched:?}"));
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        let want = format!("Commit {} files", fetched.len());
        wait_long(&mut h, "the rows ready", &|h| said(h, &want));
        shot(&mut h, "08-intake-statements");
        h.query_all_by_role_and_label(Role::Button, &want)
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        let dir = root
            .join(&folder)
            .join("Financials")
            .join("2025")
            .join("Delivery");
        wait_long(&mut h, "filed in Financials\\2025\\Delivery", &|_| {
            std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0) >= fetched.len()
        });
        let start = std::time::Instant::now();
        let types: Vec<(String, String)> = loop {
            let got: Vec<(String, String)> = {
                let o = open_mut(&mut h);
                let mut st = o
                    .opened
                    .conn
                    .prepare(
                        "SELECT d.rel_path, t.name FROM document d JOIN doc_type t ON t.id = d.doc_type_id
                         WHERE d.rel_path LIKE '%Financials/2025/Delivery/%' AND d.deleted_at IS NULL
                         ORDER BY d.rel_path",
                    )
                    .unwrap();
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap()
                    .flatten()
                    .collect()
            };
            if got.len() >= fetched.len() || start.elapsed().as_secs() > 60 {
                break got;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        };
        note(format!("Step 6: committed: {types:?}"));
        assert_eq!(types.len(), fetched.len());
        assert!(
            types
                .iter()
                .all(|(_, t)| t.starts_with("Statements delivery"))
        );
        h.state_mut().tab = crate::app::Tab::Browse;
        h.run_steps(6);

        // ---- Step 7: Balance owed… with Hold the client's files. Another file, not on the
        // paid card, is shown to Jordan first: the hold holds it and spares the statements
        // (owner 2026-11-26).
        let other = t.path().join("2025 Organizer - Sample.pdf");
        crate::ui::test_util::write_lines_pdf(&other, &[&["Sample client", "2025 organizer"]]);
        let other_id = p
            .add(
                &c.id,
                "walk-organizer",
                "2025 Organizer - Sample.pdf",
                Some(2025),
                &other,
            )
            .unwrap()
            .file_id()
            .to_string();
        let people: std::collections::BTreeMap<String, bool> = p
            .users(&c.id)
            .unwrap()
            .users
            .iter()
            .map(|u| (u.identity_id.clone(), true))
            .collect();
        p.set_visibility(&other_id, &people).unwrap();
        h.get_by_role_and_label(Role::Button, "Balance owed…")
            .click();
        wait_long(&mut h, "the balance window read", &|h| {
            said(h, "No balance on the portal.")
        });
        h.get_by_label("Balance amount").click();
        h.step();
        h.get_by_label("Balance amount").type_text("300");
        h.step();
        h.get_by_label("Balance memo").click();
        h.step();
        h.get_by_label("Balance memo").type_text("2025 compilation");
        h.run_steps(3);
        h.get_by_label("Hold the client's files until this is paid")
            .click();
        h.run_steps(3);
        shot(&mut h, "09-balance-hold");
        h.get_by_role_and_label(Role::Button, "Post to portal")
            .click();
        wait_long(&mut h, "posted", &|h| said(h, "Posted to the portal."));
        shot(&mut h, "10-balance-hold-posted");
        let b = p.client_balances(&c.id).unwrap();
        assert_eq!(b[0].hold_files, Some(true));
        note(format!(
            "Step 7: posted $300.00 \"2025 compilation\" with the hold; the portal has hold_files {:?}, state {}",
            b[0].hold_files, b[0].state
        ));
        open_mut(&mut h).browse.portal.balance = None;
        h.run_steps(4);

        // ---- Step 8: the client's files are held; they pay; the files open.
        assert!(client("held"), "see client-held.json");
        let paid = p.client_balances(&c.id).unwrap();
        assert_eq!(paid[0].state, "paid");
        note("Step 8: the client saw the files held and paid; see client-held.json".into());
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Annual agreements (ADDENDUM-annual-agreements.md, owner 2026-11-25) walkthrough against a
    /// real Vibe Portal and the stand-in Stripe: Service agreement… for a business with the
    /// bookkeeping, payroll and advisory letters (the Terms and Conditions filled in once), one
    /// one-time and two monthly fees; the client signs each letter and sets up payment by card
    /// (Checkout in subscription mode); a month is paid and the next fails; every step's PDF and
    /// the month's receipt are fetched and committed by Intake; the window shows the failed
    /// month; End engagement… sends the closure letter and stops the charge. Set `VF_WALK_URL`,
    /// `VF_WALK_KEY`, `VF_WALK_LOG`, `VF_WALK_CLIENT`, `VF_WALK_STRIPE` and `VF_WALK_DIR`.
    #[test]
    #[ignore = "needs a running portal"]
    fn agreement_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        for (_, name) in vf_portal::delivery::AGREEMENT_STEP_DOC_TYPES {
            vf_fs::doctypes::ensure(&root, name, vf_portal::delivery::AGREEMENT_FOLDER, true)
                .unwrap();
        }
        vf_fs::doctypes::ensure(
            &root,
            vf_portal::inbox::RECEIPT_DOC_TYPE,
            "Billing/{Year}",
            true,
        )
        .unwrap();
        vf_ingest::parsers::sync_filer_rules_report(&root).unwrap();
        vf_letters::store::seed_if_missing(&root).unwrap();
        // The firm fills in its Terms and Conditions once (A17).
        let mut terms = vf_letters::store::load_all(&root)
            .unwrap()
            .into_iter()
            .find(|t| t.kind == vf_letters::TERMS)
            .unwrap();
        let mut json = serde_json::to_string(&terms.blocks).unwrap();
        for (k, v) in [
            ("[X]", "30"),
            ("[Portal Name]", "Sample Portal"),
            ("[State Name]", "Ohio"),
            (
                "[Name of State or Federal Court Desired]",
                "Franklin County, Ohio",
            ),
        ] {
            json = json.replace(k, v);
        }
        terms.blocks = serde_json::from_str(&json).unwrap();
        terms.sample = false;
        vf_letters::store::save(&root, &terms).unwrap();
        vf_letters::partners::save(
            &root,
            &[vf_letters::partners::Partner {
                name: "Morgan Example".into(),
                title: "Partner".into(),
                code: "ME".into(),
            }],
        )
        .unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 120, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let folder = c.folder_rel.clone().unwrap();
        // A business, its contact the person who signs for it.
        vf_fs::clients::set_entity_type(&root, &c.id, Some("llc")).unwrap();
        for (k, v) in [
            ("first", "Pat"),
            ("last", "Sample"),
            ("email", "pat.sample@example.com"),
        ] {
            vf_fs::contacts::set_one_by_hand(&root, &c.id, k, v).unwrap();
        }
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some("pat.sample@example.com"),
            None,
            "Pat Sample",
            "Owner",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}) a business on the portal, Pat Sample its contact; the Terms and Conditions filled in; Morgan Example the partner",
            c.name, c.ext_id
        ));

        // ---- Step 1: Service agreement… with three services.
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Service agreement…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "SERVICES AND FEES") && !said(h, "Working…")
        });
        h.run_steps(4);
        for (service, once, month) in [
            ("Bookkeeping", "", "450.00"),
            ("Payroll", "", "125.00"),
            ("Advisory", "500.00", ""),
        ] {
            h.get_by_label(service).click();
            h.run_steps(3);
            for (label, v) in [("one-time", once), ("monthly", month)] {
                if v.is_empty() {
                    continue;
                }
                let name = format!("{service} {label} fee");
                h.get_by_label(&name).click();
                h.step();
                h.get_by_label(&name).type_text(v);
                h.run_steps(2);
            }
        }
        h.run_steps(4);
        assert!(said(&h, "Total $500.00 one-time · $575.00 a month"));
        shot(&mut h, "01-service-agreement");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        h.run_steps(4);
        shot(&mut h, "01b-after-send");
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        let a = p.agreements(&c.id).unwrap();
        assert_eq!(a.len(), 1);
        let sent = p.delivery(&a[0].id).unwrap();
        assert_eq!(sent.covers, ["Bookkeeping", "Payroll", "Advisory"]);
        assert_eq!(sent.letters.len(), 3);
        assert_eq!(sent.signers[0].role, "officer");
        note(format!(
            "Step 1: sent {}: {} letters (bookkeeping, payroll, advisory, each with the Terms), Advisory $500.00 once, Bookkeeping $450.00 and Payroll $125.00 a month",
            sent.title,
            sent.letters.len()
        ));

        // ---- Step 2: the client signs, sets up payment, a month is paid, the next fails.
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                "pat.sample@example.com",
                "agreement",
                &stripe,
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client's side (see client-agreement.json)");
        let now = p.delivery(&a[0].id).unwrap();
        assert_eq!(now.status, "complete");
        let subs = p.subscriptions(&c.id).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].monthly_cents, 57_500);
        assert!(subs[0].failed_at.is_some());
        note(format!(
            "Step 2: signed and set up; the monthly charge: {}",
            subs[0].words()
        ));
        click_read_again(&mut h);
        h.run_steps(20);
        shot(&mut h, "02a-after-read-again");
        wait_long(&mut h, "the failed month shown", &|h| {
            said(h, "the last payment failed")
        });
        shot(&mut h, "02-agreement-monthly-failed");

        // ---- Step 2b: Admin › Portal › Deliveries shows the failed month (owner 2026-11-25).
        open_mut(&mut h).browse.portal.agreement = None;
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Deliveries").click();
        h.run_steps(4);
        // Signed and set up, the agreement is finished: listed under its failed month, not open.
        h.get_by_label("Monthly charge failed").click();
        wait_long(&mut h, "the failed month on the Deliveries page", &|h| {
            said(h, &c.name) && said(h, "Failed ")
        });
        assert!(said(&h, "Done "), "the agreement shows the day it was done");
        shot(&mut h, "02b-deliveries-monthly-failed");
        note("Step 2b: Admin › Portal › Deliveries, Monthly charge failed: the client's agreement, Done, its charge Failed".into());
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);

        // ---- Step 3: the step PDFs and the month's receipt are fetched and committed.
        open_mut(&mut h).browse.portal.agreement = None;
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let count = |inbox: &std::path::Path| {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.contains(" Agreement ") || n.contains("Payment receipt")
                })
                .count()
        };
        wait_long(&mut h, "the PDFs fetched", &|_| count(&inbox) == 3);
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        wait_long(&mut h, "the rows ready", &|h| said(h, "Commit 3 files"));
        shot(&mut h, "03-intake-agreement-pdfs");
        h.query_all_by_role_and_label(Role::Button, "Commit 3 files")
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        let year = now.tax_year;
        let eng = root
            .join(&folder)
            .join("Correspondence")
            .join(year.to_string())
            .join("Engagement");
        for want in [
            format!("{year} Agreement 1 - Letters.pdf"),
            format!("{year} Agreement 4 - Fee.pdf"),
        ] {
            let path = eng.join(&want);
            wait_long(&mut h, &want, &|_| path.exists());
        }
        let billing = root.join(&folder).join("Billing");
        let receipt = std::fs::read_dir(&billing)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|y| std::fs::read_dir(y.path()).into_iter().flatten().flatten())
            .find(|f| f.file_name().to_string_lossy().contains("Monthly services"))
            .map(|f| f.path());
        assert!(receipt.is_some(), "the monthly receipt in Billing");
        note(format!(
            "Step 3: committed into {folder}\\Correspondence\\{year}\\Engagement (the signed letters, the payment record) and the month's receipt into {}",
            receipt.unwrap().strip_prefix(&root).unwrap().display()
        ));

        // ---- Step 4: End engagement… sends the closure letter and stops the charge.
        h.state_mut().tab = crate::app::Tab::Browse;
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "End engagement…")
            .click();
        h.run_steps(6);
        for (k, v) in [
            ("[Date of Closure Letter]", "November 25, 2026"),
            ("[Date]", "November 25, 2026"),
            ("[Name of Engagement]", "Bookkeeping, payroll and advisory"),
            ("[Telephone Number]", "555-0100"),
            (
                "[and Specified Parties, optional if applicable]",
                "and its lender",
            ),
        ] {
            // The box (named as its label beside it).
            let Some(field) = h.query_all_by_label(k).last() else {
                continue;
            };
            field.click();
            h.step();
            h.query_all_by_label(k).last().unwrap().type_text(v);
            h.run_steps(2);
        }
        shot(&mut h, "04-end-engagement");
        h.get_by_role_and_label(Role::Button, "Send and end engagement…")
            .click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "End engagement")
            .click();
        wait_long(&mut h, "ended", &|h| {
            said(h, "closure letter is on the client's portal")
        });
        shot(&mut h, "05-engagement-ended");
        let subs = p.subscriptions(&c.id).unwrap();
        assert!(subs[0].canceled_at.is_some());
        let files = p.files_for_client(&c.id).unwrap();
        assert!(
            files
                .iter()
                .any(|f| f.name.contains("Engagement closure letter"))
        );
        note("Step 4: the closure letter shared on the portal and in the Portal inbox; the monthly charge stopped".into());
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Annual agreements, the second walk (2026-11-25): a joint couple (both sign, the spouse by
    /// a code) paying monthly by bank account (started, then cleared); Deliver… for
    /// the return the agreement covers skips its letters (A11); staff change the monthly amount;
    /// Admin › Portal › Agreements sends next year's agreement from the last one with the changed
    /// amount carried over; the couple signs in again and signs it, paying only its one-time fee
    /// while the monthly charge continues; staff stop the charge; a Sign only agreement is sent
    /// and cancelled. Same environment as `agreement_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn agreement_joint_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(stripe), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        vf_letters::store::seed_if_missing(&root).unwrap();
        // The firm fills in the kit's Form 1040 letter and its Terms and Conditions (A17).
        for id in ["aicpa-1040", "aicpa-terms"] {
            let mut tpl = vf_letters::store::load_all(&root)
                .unwrap()
                .into_iter()
                .find(|t| t.id == id)
                .unwrap();
            let mut json = serde_json::to_string(&tpl.blocks).unwrap();
            for (k, v) in [
                ("[X]", "30"),
                ("[Portal Name]", "Sample Portal"),
                ("[State Name]", "Ohio"),
                (
                    "[Name of State or Federal Court Desired]",
                    "Franklin County, Ohio",
                ),
                ("[Date]", "December 31, {Tax year}"),
                ("[Frequency]", "quarterly"),
                ("[Local]", "City of Columbus"),
                ("[Number]", "1040"),
                ("[State]", "Ohio"),
                (
                    "[If a large number of returns are included, consider listing the due dates in an appendix]",
                    "April 15, {Next year}",
                ),
            ] {
                json = json.replace(k, v);
            }
            tpl.blocks = serde_json::from_str(&json).unwrap();
            tpl.sample = false;
            vf_letters::store::save(&root, &tpl).unwrap();
        }
        vf_letters::partners::save(
            &root,
            &[vf_letters::partners::Partner {
                name: "Morgan Example".into(),
                title: "Partner".into(),
                code: "ME".into(),
            }],
        )
        .unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 120, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(1));
            o.browse.portal.reread();
            o.browse.snap.clients[1].clone()
        };
        for (k, v) in [
            ("first", "Jordan"),
            ("last", "Sample"),
            ("email", "jordan.sample@example.com"),
            ("spouse_first", "Casey"),
            ("spouse_last", "Sample"),
            ("spouse_email", "casey.sample@example.com"),
        ] {
            vf_fs::contacts::set_one_by_hand(&root, &c.id, k, v).unwrap();
        }
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Taxpayer",
        )
        .unwrap();
        note(format!(
            "Setup: {} ({}), an individual: Jordan and Casey Sample; the 1040 letter and the Terms filled in",
            c.name, c.ext_id
        ));
        let open_agreement = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
                              year: Option<i32>| {
            open_mut(h).browse.portal.agreement = None;
            h.run_steps(3);
            h.get_by_role_and_label(Role::Button, "Service agreement…")
                .click();
            h.run_steps(2);
            if let Some(y) = year {
                open_mut(h)
                    .browse
                    .portal
                    .agreement
                    .as_mut()
                    .unwrap()
                    .set_year(y);
            }
            let start = std::time::Instant::now();
            while !(h
                .query_all_by_label_contains("SERVICES AND FEES")
                .next()
                .is_some()
                && h.query_all_by_label_contains("Working…").next().is_none())
            {
                assert!(
                    start.elapsed().as_secs() < 60,
                    "the agreement window: never"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
                h.step();
            }
            h.run_steps(4);
        };
        let type_into =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, v: &str| {
                h.get_by_label(label).click();
                h.step();
                h.key_press(egui::Key::End);
                for _ in 0..12 {
                    h.key_press(egui::Key::Backspace);
                }
                h.step();
                h.get_by_label(label).type_text(v);
                h.run_steps(2);
            };

        // ---- Step 1: a 2026 agreement (it covers the 2025 return): 1040 once, bookkeeping monthly.
        open_agreement(&mut h, Some(2026));
        for (service, label, v) in [
            ("1040", "1040 one-time fee", "640.00"),
            ("Bookkeeping", "Bookkeeping monthly fee", "100.00"),
        ] {
            h.get_by_label(service).click();
            h.run_steps(3);
            type_into(&mut h, label, v);
        }
        h.run_steps(4);
        assert!(
            said(&h, "Total $640.00 one-time · $100.00 a month"),
            "the totals"
        );
        assert!(said(&h, "(taxpayer and spouse)"));
        shot(&mut h, "10-joint-agreement");
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent to the client."));
        let first = p.agreements(&c.id).unwrap()[0].clone();
        let first = p.delivery(&first.id).unwrap();
        assert_eq!(
            first
                .signers
                .iter()
                .map(|s| s.role.as_str())
                .collect::<Vec<_>>(),
            ["taxpayer", "spouse"]
        );
        assert_eq!(first.letters.len(), 2);
        assert_eq!(first.return_year, Some(2025));
        note(format!(
            "Step 1: sent {}: the 1040 and bookkeeping letters, both spouses to sign; 1040 $640.00 once, bookkeeping $100.00 a month",
            first.title
        ));

        // ---- Step 2: both sign; set up by bank account; started, then cleared.
        let run_client = |phase: &str| {
            std::process::Command::new("node")
                .args([
                    client_js.as_str(),
                    url.as_str(),
                    &out.display().to_string(),
                    log.as_str(),
                    "jordan.sample@example.com",
                    phase,
                    "casey.sample@example.com",
                    stripe.as_str(),
                ])
                .status()
                .is_ok_and(|s| s.success())
        };
        assert!(
            run_client("agreement-joint"),
            "see client-agreement-joint.json"
        );
        let now = p.delivery(&first.id).unwrap();
        assert_eq!(now.status, "complete");
        let sub = p.subscriptions(&c.id).unwrap()[0].clone();
        assert_eq!((sub.method.as_str(), sub.monthly_cents), ("bank", 10_000));
        note(format!(
            "Step 2: both signed (Casey by a code); bank payment started, then cleared; {}",
            sub.words()
        ));

        // ---- Step 3: Deliver… for the 2025 1040 says the engagement is signed.
        open_mut(&mut h).browse.portal.agreement = None;
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "the A11 note", &|h| {
            said(h, "Engagement signed in the 2026 service agreement")
        });
        shot(&mut h, "11-deliver-skips-letters");
        open_mut(&mut h).browse.portal.delivery = None;
        note("Step 3: Deliver… for the 2025 1040: \"Engagement signed in the 2026 service agreement\", letters unticked".into());

        // ---- Step 4: staff change the monthly amount from the next charge.
        open_agreement(&mut h, Some(2026));
        assert!(said(&h, "$100.00 a month by bank account"));
        type_into(&mut h, "New monthly amount", "120.00");
        h.get_by_role_and_label(Role::Button, "Change from the next charge")
            .click();
        wait_long(&mut h, "changed", &|h| said(h, "Changed to $120.00"));
        assert_eq!(p.subscriptions(&c.id).unwrap()[0].monthly_cents, 12_000);
        note(
            "Step 4: the monthly charge changed to $120.00 from the next charge; the client told"
                .into(),
        );

        // ---- Step 5: Admin › Portal › Agreements sends the 2027 agreement from the last one.
        open_mut(&mut h).browse.portal.agreement = None;
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Agreements").click();
        h.run_steps(4);
        {
            let a = open_mut(&mut h).admin.as_mut().unwrap();
            a.agreements.year = 2027;
            a.agreements.reread();
        }
        let tick = format!("{} ({})", c.name, c.ext_id);
        // Settled: the portal's agreements and monthly charges are both in (the row shows the
        // changed amount), so the rows no longer move under the click.
        wait_long(&mut h, "the bulk list", &|h| {
            h.query_by_label(&tick).is_some()
                && !said(h, "Reading the portal")
                && said(h, "$120.00")
        });
        h.run_steps(4);
        h.get_by_label(&tick).click();
        h.run_steps(3);
        assert!(said(&h, "1 ticked"), "the client ticked");
        shot(&mut h, "12-bulk-agreements");
        h.get_by_role_and_label(Role::Button, "Send 1 agreement")
            .click();
        wait_long(&mut h, "bulk sent", &|h| said(h, "Sent 1 of 1 agreements."));
        let second = p
            .agreements(&c.id)
            .unwrap()
            .into_iter()
            .find(|d| d.tax_year == 2027)
            .unwrap();
        let second = p.delivery(&second.id).unwrap();
        let lines: Vec<(String, u64, String)> = second
            .fee
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|l| (l.name.clone(), l.cents, l.every.clone()))
            .collect();
        assert_eq!(
            lines,
            [
                ("1040".to_string(), 64_000, "once".to_string()),
                ("Bookkeeping".to_string(), 12_000, "month".to_string())
            ]
        );
        note("Step 5: Admin › Portal › Agreements sent the 2027 agreement: 1040 $640.00 once, bookkeeping $120.00 a month (the changed amount)".into());

        // ---- Step 6: the couple signs in again, signs, pays the one-time fee only.
        assert!(run_client("renew"), "see client-renew.json");
        let now = p.delivery(&second.id).unwrap();
        assert_eq!(now.status, "complete");
        let subs = p.subscriptions(&c.id).unwrap();
        assert_eq!(subs.len(), 1, "the same monthly charge, not a second one");
        assert_eq!(
            (subs[0].monthly_cents, subs[0].delivery_id.as_str()),
            (12_000, second.id.as_str())
        );
        note(format!(
            "Step 6: the 2027 agreement signed; the one-time fee paid by card; the monthly charge continues ({})",
            subs[0].words()
        ));

        // ---- Step 7: staff stop the monthly charge.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        open_agreement(&mut h, Some(2027));
        h.get_by_role_and_label(Role::Button, "Stop monthly charge…")
            .click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Stop it").click();
        wait_long(&mut h, "stopped", &|h| {
            said(h, "The monthly charge was stopped")
        });
        assert!(p.subscriptions(&c.id).unwrap()[0].canceled_at.is_some());
        note("Step 7: the monthly charge stopped from the window; the client told".into());

        // ---- Step 8: a Sign only agreement, sent and cancelled.
        open_agreement(&mut h, Some(2028));
        h.get_by_label("Bookkeeping").click();
        h.run_steps(3);
        h.get_by_label("Sign only").click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Send to client")
            .click();
        wait_long(&mut h, "sign only sent", &|h| {
            said(h, "Sent to the client.")
        });
        let third = p
            .agreements(&c.id)
            .unwrap()
            .into_iter()
            .find(|d| d.tax_year == 2028)
            .unwrap();
        assert!(third.step("fee").is_none_or(|s| !s.present));
        h.get_by_role_and_label(Role::Button, "Cancel agreement…")
            .click();
        h.run_steps(3);
        h.get_by_role_and_label(Role::Button, "Cancel agreement")
            .click();
        wait_long(&mut h, "cancelled", &|h| {
            said(h, "The agreement was cancelled.")
        });
        assert_eq!(p.delivery(&third.id).unwrap().status, "cancelled");
        shot(&mut h, "13-sign-only-cancelled");
        note("Step 8: a Sign only 2028 agreement (no payment step) sent, then cancelled".into());
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Presses the open window's Read again.
    fn click_read_again(h: &mut egui_kittest::Harness<'static, VibeFilesApp>) {
        h.query_all_by_role_and_label(Role::Button, "Read again")
            .last()
            .unwrap()
            .click();
        h.run_steps(4);
    }

    /// Owner 2026-11-26 against a real Vibe Portal: Admin › Portal's Assign new cards to (Pat
    /// Preparer, chosen and saved), Balance owed… and Deliver… starting with Pat; the 1040 kit
    /// letter's fill-ins typed for one client; Admin › Portal › Balances sorted and filtered.
    /// The same `VF_WALK_*` variables as `balance_walkthrough`.
    #[test]
    #[ignore = "needs a running portal"]
    fn new_cards_walkthrough() {
        let (Ok(url), Ok(key), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_balances(true)
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        let clients = open_mut(&mut h).browse.snap.clients.clone();
        let p = crate::portal::client(&s);
        for c in &clients {
            p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        }
        // Two balances posted for the grid (by the API: the windows are step 2's).
        for (c, cents, memo) in [
            (&clients[1], 45_000u64, "2025 bookkeeping"),
            (&clients[2], 7_500, "Amended 2024"),
        ] {
            p.post_balance(
                &c.id,
                &vf_portal::balance::PostBalance {
                    amount_cents: cents,
                    memo: memo.into(),
                    expire_days: None,
                    notify: false,
                    remind_days: 0,
                    assigned_staff_id: None,
                    hold_files: false,
                },
            )
            .unwrap();
        }
        note(format!(
            "Setup: {} clients on the portal; balances for {} and {}",
            clients.len(),
            clients[1].name,
            clients[2].name
        ));

        // ---- Step 1: Admin › Portal, Assign new cards to Pat Preparer, Save.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(6);
        wait_long(&mut h, "the staff list", &|h| {
            h.query_by_label("Assign new cards to").is_some()
        });
        h.run_steps(20);
        h.get_by_label("Assign new cards to").click();
        h.run_steps(3);
        wait_long(&mut h, "Pat in the list", &|h| said(h, "Pat Preparer"));
        h.get_by_label("Pat Preparer").click();
        h.run_steps(3);
        shot(&mut h, "01-assign-new-cards-to");
        // Save is below the fold: scrolled into view, as a person would.
        h.get_by_role_and_label(Role::Button, "Save").scroll_to_me();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Save").click();
        h.run_steps(6);
        shot(&mut h, "01b-saved");
        let saved = vf_portal::settings::load(&root, &cab).unwrap();
        assert_eq!(
            saved.default_staff.as_ref().map(|d| d.name.as_str()),
            Some("Pat Preparer")
        );
        note("Step 1: Admin › Portal, Assign new cards to Pat Preparer, saved for every PC".into());

        // ---- Step 2: Balance owed… and Deliver… start with Pat.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
        }
        h.run_steps(8);
        wait_long(&mut h, "the panel's buttons", &|h| {
            h.query_by_role_and_label(Role::Button, "Balance owed…")
                .is_some()
        });
        h.get_by_role_and_label(Role::Button, "Balance owed…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "No balance on the portal.")
        });
        let pat = saved.default_staff.as_ref().unwrap().id.clone();
        wait_long(&mut h, "Pat assigned", &|h| {
            let crate::app::Screen::Open(o) = &h.state().screen else {
                return false;
            };
            o.browse.portal.balance.as_ref().and_then(|w| w.assigned()) == Some(pat.as_str())
        });
        shot(&mut h, "02-balance-window-pat");
        open_mut(&mut h).browse.portal.balance = None;
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Deliver…").click();
        wait_long(&mut h, "Pat assigned in Deliver", &|h| {
            let crate::app::Screen::Open(o) = &h.state().screen else {
                return false;
            };
            o.browse.portal.delivery.as_ref().and_then(|w| w.assigned()) == Some(pat.as_str())
        });
        shot(&mut h, "02b-deliver-window-pat");
        note(
            "Step 2: a new Balance owed… and a new Deliver… start with Assigned to Pat Preparer"
                .into(),
        );

        // ---- Step 3: the 1040 kit letter's fill-ins typed for this client.
        h.get_by_label("Letters to sign").click();
        h.run_steps(4);
        h.query_all_by_label_contains("individual income tax return (Form 1040)")
            .next()
            .expect("the kit's 1040 letter offered")
            .click();
        h.run_steps(4);
        wait_long(&mut h, "the boxes", &|h| said(h, "Fill in for this client"));
        let left_before = h.query_all_by_label("left").count();
        assert!(left_before > 0, "the kit letter has fill-ins");
        h.get_by_label("[State]").click();
        h.step();
        h.get_by_label("[State]").type_text("Kansas");
        h.run_steps(4);
        let left_after = h.query_all_by_label("left").count();
        assert_eq!(
            left_after + 1,
            left_before,
            "the typed one is no longer left"
        );
        shot(&mut h, "03-fill-ins-typed");
        note(format!(
            "Step 3: the 1040 kit letter ticked shows Fill in for this client with {left_before} boxes; [State] typed, {left_after} left"
        ));
        open_mut(&mut h).browse.portal.delivery = None;
        h.run_steps(4);

        // ---- Step 4: Admin › Portal › Balances: sorted by Amount both ways, filtered.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Balances").click();
        wait_long(&mut h, "the balances", &|h| said(h, "$75.00"));
        h.run_steps(6);
        let top = |h: &egui_kittest::Harness<'static, VibeFilesApp>, l: &str| {
            h.get_by_label(l).rect().top()
        };
        h.get_by_label("Amount").click();
        h.run_steps(4);
        assert!(top(&h, "$75.00") < top(&h, "$450.00"));
        h.get_by_label("Amount").click();
        h.run_steps(4);
        assert!(top(&h, "$450.00") < top(&h, "$75.00"));
        shot(&mut h, "04-balances-sorted");
        h.get_by_label("Filter").click();
        h.step();
        h.get_by_label("Filter").type_text("amended");
        h.run_steps(4);
        assert!(said(&h, "1 of 2"));
        assert!(h.query_by_label("$450.00").is_none());
        shot(&mut h, "05-balances-filtered");
        note("Step 4: Admin › Portal › Balances sorted by Amount up and down; Filter amended: 1 of 2".into());
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// Addendum §16 (balance owed) walkthrough against a real Vibe Portal with a stand-in for
    /// Stripe: post from the Balance owed window, the payment completed by a signed webhook,
    /// the receipt fetched into the Portal inbox with its hint, the panel's line, the copy in
    /// the client folder, the audit row, and where Intake's rules file the receipt. Set
    /// `VF_WALK_URL`, `VF_WALK_KEY`, `VF_WALK_STRIPE` (the stand-in, with `/_pay` and `/_last`),
    /// `VF_WALK_LOG` (the portal's log), `VF_WALK_CLIENT` (`scripts/balance-walkthrough/client.mjs`)
    /// and `VF_WALK_DIR` (output).
    #[test]
    #[ignore = "needs a running portal"]
    fn balance_walkthrough() {
        let (Ok(url), Ok(key), Ok(stripe), Ok(log), Ok(client_js), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_STRIPE"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 3).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_balances(true);
        vf_portal::settings::save(&root, &s).unwrap();
        crate::portal::register_inbox(&root).unwrap();
        // What Admin › Portal's Save does when Balance owed is ticked.
        assert!(
            vf_fs::doctypes::ensure(
                &root,
                vf_portal::inbox::RECEIPT_DOC_TYPE,
                "Billing/{Year}",
                true
            )
            .unwrap()
        );
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    assert!(start.elapsed().as_secs() < 90, "{what}: never");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
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
        // The doc type reaches the catalog as Admin's Save syncs it.
        {
            let o = open_mut(&mut h);
            vf_fs::doctypes::sync(&mut o.opened.conn, &root, "dev", "doc_types_edit").unwrap();
        }
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        // Someone of the client on the portal (a payment is always made by one, signed in).
        p.invite(
            &c.id,
            Some("jordan.sample@example.com"),
            None,
            "Jordan Sample",
            "Owner",
        )
        .unwrap();
        h.run_steps(6);
        {
            let start = std::time::Instant::now();
            while start.elapsed().as_secs() < 60 && !said(&h, "Balance owed: none") {
                std::thread::sleep(std::time::Duration::from_millis(50));
                h.step();
            }
            if !said(&h, "Balance owed: none") {
                shot(&mut h, "00-panel-line-missing");
                use egui_kittest::kittest::NodeT;
                let seen: Vec<String> = h
                    .query_all_by_label_contains("Balance owed")
                    .filter_map(|n| n.accesskit_node().label())
                    .collect();
                panic!("the panel's line: {seen:?}");
            }
        }
        note(format!(
            "Setup: {} ({}) on the portal; the panel says Balance owed: none",
            c.name, c.ext_id
        ));

        // ---- Step 1: post a balance from the client panel.
        h.get_by_role_and_label(Role::Button, "Balance owed…")
            .click();
        wait_long(&mut h, "the window read", &|h| {
            said(h, "No balance on the portal.")
        });
        h.get_by_label("Balance amount").click();
        h.step();
        h.get_by_label("Balance amount").type_text("240");
        h.step();
        h.get_by_label("Balance memo").click();
        h.step();
        h.get_by_label("Balance memo")
            .type_text("2025 bookkeeping, Oct–Dec");
        h.run_steps(3);
        shot(&mut h, "01-balance-window-filled");
        h.get_by_role_and_label(Role::Button, "Post to portal")
            .click();
        wait_long(&mut h, "posted", &|h| said(h, "Posted to the portal."));
        shot(&mut h, "02-balance-posted");
        let posted = p.client_balances(&c.id).unwrap();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].amount_cents, 24_000);
        assert_eq!(posted[0].state, "due");
        // The firm's default (Settings › Payments): leaves after 14 days.
        assert!(posted[0].expires_at.is_some(), "the box started ticked");
        note(format!(
            "Step 1: posted $240.00 \"2025 bookkeeping, Oct–Dec\"; the portal has it due, leaving {}",
            posted[0].expires_at.clone().unwrap_or_default()
        ));
        let bid = posted[0].id.clone();
        // Closed: the panel's line follows.
        h.query_all_by_label("Close window").for_each(|_| ());
        {
            let o = open_mut(&mut h);
            o.browse.portal.balance = None;
            o.browse.portal.look_at_balances_now();
        }
        wait_long(&mut h, "panel line due", &|h| {
            said(h, "$240.00 on the portal")
        });
        shot(&mut h, "03-panel-line-due");
        note(
            "Step 2: the client panel says Balance owed: $240.00 on the portal, N days left".into(),
        );

        // ---- Step 3: the client pays by card on the portal: accepts the invitation, presses
        // Pay on the Balance due card, pays on Stripe's page (the stand-in completes the session
        // and signs the webhook as Stripe does), and comes back to the card.
        let ok = std::process::Command::new("node")
            .args([
                &client_js,
                &url,
                &out.display().to_string(),
                &log,
                "jordan.sample@example.com",
                "pay",
                &stripe,
            ])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "the client pays (see client-pay.json)");
        let paid = p.client_balances(&c.id).unwrap();
        assert_eq!(paid[0].id, bid);
        assert_eq!(paid[0].state, "paid");
        assert_eq!(paid[0].method.as_deref(), Some("card"));
        assert!(paid[0].receipt_upload_id.is_some(), "a receipt was filed");
        note("Step 3: the client paid by card (Pay, Stripe's page, back to the card: Paid with a Receipt); the portal filed the receipt".into());

        // ---- Step 4: the receipt is fetched into the Portal inbox, with its hint.
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = crate::portal::inbox(&root);
        let found = |inbox: &std::path::Path| {
            std::fs::read_dir(inbox)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .find(|n| n.contains("Payment receipt"))
        };
        wait_long(&mut h, "the receipt fetched", &|_| found(&inbox).is_some());
        let name = found(&inbox).unwrap();
        assert!(
            name.ends_with("Payment receipt - 2025 bookkeeping, Oct–Dec.pdf"),
            "{name}"
        );
        let bytes = std::fs::read(inbox.join(&name)).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let sha = filer_intake::commit::sha256_file(&inbox.join(&name)).unwrap();
        let hint = vf_fs::hints::load(&root)
            .get(&sha)
            .cloned()
            .expect("a hint for Intake");
        let types = vf_fs::doctypes::load(&root).unwrap().unwrap();
        let receipt_type = vf_fs::doctypes::id_by_name(&types, "Payment receipt").unwrap();
        assert_eq!(hint.doc_type.as_deref(), Some(receipt_type.as_str()));
        assert_eq!(hint.origin, "portal_balance");
        note(format!(
            "Step 4: {name} reached the Portal inbox; Intake's hint gives it the doc type Payment receipt"
        ));

        // ---- Step 5: the panel says paid; the copy and the audit row.
        open_mut(&mut h).browse.portal.look_at_balances_now();
        wait_long(&mut h, "panel line paid", &|h| {
            said(h, "Balance owed: none")
        });
        let folder = c.folder_rel.clone().unwrap();
        let copy = vf_portal::balance::read_copy(&root, &folder);
        assert_eq!(copy.len(), 1);
        assert_eq!(copy[0].balance.state, "paid");
        let n: i64 = open_mut(&mut h)
            .opened
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'portal_balance_paid'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let posts: i64 = open_mut(&mut h)
            .opened
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'portal_balance_post'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(posts, 1);
        note("Step 5: the panel says Balance owed: none; _Vibe/balance.jsonl has the paid balance; audit has portal_balance_post and portal_balance_paid".into());

        // ---- Step 6: Intake's rules file it in Billing/<year paid>.
        let master = vf_fs::clients::master_dir(&root);
        let (rules, _, _) = filer_core::rules::load_or_seed(&master);
        let compiled = filer_core::rules::CompiledRules::new(rules.active_rules()).unwrap();
        let rest = name
            .trim_end_matches(".pdf")
            .rsplit_once(&format!("_{}_", c.ext_id))
            .map(|(_, r)| r.to_string())
            .unwrap_or_else(|| name.clone());
        let hit = compiled
            .first_specific_match(&rest, false)
            .expect("a rule takes it");
        assert_eq!(hit.dest_template, "{Client}/Billing/{Year}");
        let year = filer_core::parse::detect_year(&rest, 2026).unwrap();
        note(format!(
            "Step 6: Intake's rule \"{}\" files it in {}/Billing/{year}",
            hit.name, folder
        ));
        h.state_mut().tab = crate::app::Tab::Intake;
        h.run_steps(10);
        if let Some(b) = h.query_by_label("Preview") {
            b.click();
        }
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 30 && !said(&h, "Commit 1 file") {
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.step();
        }
        shot(&mut h, "04-intake-receipt-row");
        // The row is ready (rule, client and the year paid); its destination, painted text,
        // is in the screenshot: …\Billing\<year>\<date> Payment receipt - <memo>.pdf.
        assert!(
            said(&h, "Commit 1 file"),
            "Intake's receipt row is ready (see 04-intake-receipt-row.png)"
        );
        note(format!(
            "Step 6: Intake's preview row: rule Payment receipt, year {year} (the year paid, from its hint), into Billing; ready to commit"
        ));

        // ---- Step 7: committed: the receipt is in Billing\<year paid>, a Payment receipt.
        h.query_all_by_role_and_label(Role::Button, "Commit 1 file")
            .last()
            .unwrap()
            .click();
        h.run_steps(6);
        // Intake's commit summary, then Confirm commit.
        wait_long(&mut h, "the commit summary", &|h| {
            h.query_all_by_label("Confirm commit").next().is_some()
        });
        shot(&mut h, "05-commit-summary");
        h.query_all_by_label("Confirm commit")
            .next()
            .unwrap()
            .click();
        h.run_steps(6);
        let filed_dir = root.join(&folder).join("Billing").join(year.to_string());
        let want = rest.clone() + ".pdf";
        wait_long(&mut h, "the receipt filed in Billing", &|_| {
            filed_dir.join(&want).exists()
        });
        // Recorded by Intake's catalog step (with the hint's doc type) on a later frame.
        let start = std::time::Instant::now();
        let doc_type_name = loop {
            let got: Option<String> = open_mut(&mut h)
                .opened
                .conn
                .query_row(
                    "SELECT t.name FROM document d JOIN doc_type t ON t.id = d.doc_type_id
                     WHERE d.rel_path LIKE ?1 AND d.deleted_at IS NULL",
                    [format!("%Billing/{year}/%Payment receipt%")],
                    |r| r.get(0),
                )
                .ok();
            if got.is_some() || start.elapsed().as_secs() > 60 {
                break got;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.step();
        };
        assert_eq!(doc_type_name.as_deref(), Some("Payment receipt"));
        note(format!(
            "Step 7: committed: {folder}\\Billing\\{year}\\{want}, doc type Payment receipt in the catalog"
        ));
        std::fs::write(out.join("steps.txt"), steps.join("\n")).unwrap();
    }

    /// PA-103 walkthrough against a real Vibe Portal (not a fake): the to-do window drives every
    /// firm step, and `node client.mjs` the client's steps in a browser. Set `VF_WALK_URL`,
    /// `VF_WALK_KEY`, `VF_WALK_LOG` (the portal's log, EMAIL_PROVIDER=console), `VF_WALK_CLIENT`
    /// (`scripts/todo-walkthrough/client.mjs`), `VF_WALK_PDF` (the file the client sends) and `VF_WALK_DIR` (output).
    #[test]
    #[ignore = "needs a running portal"]
    fn todo_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(pdf), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_PDF"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        // A new address each run: the portal keeps the people of earlier runs.
        let email = &format!(
            "jordan.sample+{}@example.com",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        let staff_email = "pat.preparer@example.com";
        let mut steps: Vec<String> = Vec::new();
        // Only messages from this run count (the log may hold earlier runs').
        let since_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let client = |phase: &str| -> bool {
            std::process::Command::new("node")
                .args([
                    &client_js,
                    &url,
                    &out.display().to_string(),
                    &log,
                    email,
                    phase,
                    &pdf,
                ])
                .status()
                .is_ok_and(|s| s.success())
        };
        // Messages to `to` in the portal's log, newest last: (subject, text).
        let mail = |to: &str| -> Vec<(String, String)> {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains("\"msg\":\"notify ("))
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|m| m["to"] == to && m["time"].as_i64().is_some_and(|t| t >= since_ms))
                .map(|m| {
                    (
                        m["subject"].as_str().unwrap_or("").to_string(),
                        m["body"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        };

        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 5).unwrap();
        crate::ui::test_util::pdfium_first();
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !name.starts_with('_') {
                let dir = e.path().join("Income Tax").join("2025");
                std::fs::create_dir_all(&dir).unwrap();
                crate::ui::test_util::write_lines_pdf(
                    &dir.join("2025 1040 Tax Return.pdf"),
                    crate::ui::test_util::SAMPLE_RETURN,
                );
            }
        }
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_todo_lists(true);
        vf_portal::settings::save(&root, &s).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 900.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let click_last = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str| {
            let n = h.query_all_by_role_and_label(Role::Button, label).count();
            assert!(n > 0, "no button {label:?}");
            h.query_all_by_role_and_label(Role::Button, label)
                .last()
                .unwrap()
                .scroll_to_me();
            h.run_steps(2);
            h.query_all_by_role_and_label(Role::Button, label)
                .last()
                .unwrap()
                .click();
            h.run_steps(4);
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_by_label_contains(text).is_some()
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
        let c = {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
            o.browse.snap.clients[0].clone()
        };
        h.run_steps(6);
        // The client's person is invited to the portal (the call Portal users… makes), and
        // accepts: a client already on the portal.
        let p = crate::portal::client(&s);
        p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
        p.invite(&c.id, Some(email), None, "Jordan Sample", "Owner")
            .unwrap();
        note(format!(
            "Setup: {} ({}) invited {email} to the portal",
            c.name, c.ext_id
        ));
        assert!(client("accept"), "the client accepts the invitation");

        // ---- Step 1: create a to-do list (from last year's return, PA-104).
        h.get_by_role_and_label(Role::Button, "To-do lists…")
            .click();
        assert!(wait_for(&mut h, |h| h
            .query_by_label("+ New list")
            .is_some()));
        h.run_steps(6);
        click_last(&mut h, "From last year's return…");
        {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::RadioButton)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l.ends_with("2025 1040 Tax Return.pdf"))
                })
                .expect("the return is offered")
                .click();
        }
        h.run_steps(4);
        click_last(&mut h, "Read it");
        assert!(wait_for(&mut h, |h| h
            .query_by_label("Items read from last year's return")
            .is_some()));
        h.run_steps(4);
        shot(&mut h, "01-vf-items-read-from-return");
        let use_label = h
            .query_all_by_role(Role::Button)
            .filter_map(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .filter(|l| l.starts_with("Use these "))
            })
            .next()
            .unwrap();
        click_last(&mut h, &use_label);
        h.run_steps(6);
        // Assigned to Pat Preparer: told by email when the client answers (step 8).
        {
            use egui_kittest::kittest::NodeT;
            let combos: Vec<String> = h
                .query_all_by_role(Role::ComboBox)
                .map(|n| {
                    let a = n.accesskit_node();
                    format!("{:?}={:?}", a.label(), a.value())
                })
                .collect();
            eprintln!("combo boxes: {combos:?}");
            h.query_all_by_role(Role::ComboBox)
                .find(|n| {
                    let a = n.accesskit_node();
                    a.value().as_deref() == Some("No one") || a.label().as_deref() == Some("No one")
                })
                .expect("the Assigned to picker")
                .click();
        }
        h.run_steps(4);
        h.get_by_label("Pat Preparer").click();
        h.run_steps(4);
        shot(&mut h, "02-vf-new-list-filled");
        note(format!(
            "Step 1: list made from the return: {use_label}, assigned to Pat Preparer"
        ));

        // ---- Step 2: send it.
        click_last(&mut h, "Send the list");
        assert!(wait_for(&mut h, |h| h
            .query_by_label_contains("1. W-2 from")
            .is_some()));
        h.run_steps(6);
        shot(&mut h, "03-vf-list-sent");
        let sent = mail(email);
        note(format!(
            "Step 2: sent; the client was emailed: {:?}",
            sent.last().map(|m| &m.0)
        ));
        assert!(
            sent.iter()
                .any(|m| m.1.contains("has sent you a to-do list"))
        );

        // ---- Step 3: edit it (remove the first item, save).
        click_last(&mut h, "Edit");
        assert!(wait_for(&mut h, |h| h
            .query_by_role_and_label(Role::Button, "Save")
            .is_some()));
        h.query_all_by_role_and_label(Role::Button, "Remove")
            .next()
            .unwrap()
            .click();
        h.run_steps(4);
        shot(&mut h, "03b-vf-editing");
        click_last(&mut h, "Save");
        assert!(wait_for(&mut h, |h| said(h, "Saved.")));
        h.run_steps(4);
        shot(&mut h, "03c-vf-edited");
        note("Step 3: edited (first item removed) and saved".into());

        // ---- Step 4: a reminder.
        click_last(&mut h, "Remind now");
        assert!(wait_for(&mut h, |h| said(h, "reminded") || said(h, "Nobody could")));
        h.run_steps(4);
        shot(&mut h, "04-vf-reminded");
        note(format!(
            "Step 4: reminder; the client was emailed: {:?}",
            mail(email).last().map(|m| &m.0)
        ));
        assert!(mail(email).iter().any(|m| m.1.contains("still open on")));

        // ---- Steps 5-7: the client signs in, uploads, answers (browser).
        let answered_at = std::time::Instant::now();
        assert!(client("answer"), "the client's browser steps");
        note(
            "Steps 5-7: client signed in, uploaded a W-2, answered, marked one not applicable"
                .into(),
        );

        // ---- Step 8: Pat Preparer is emailed (the portal gathers answers for 5 minutes).
        let mut told = None;
        while answered_at.elapsed() < std::time::Duration::from_secs(8 * 60) {
            if let Some(m) = mail(staff_email).last() {
                told = Some(m.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            h.step();
        }
        let told = told.expect("the assigned staff member is told");
        note(format!(
            "Step 8: after {} s, {staff_email} was emailed: subject {:?} / text {:?}",
            answered_at.elapsed().as_secs(),
            told.0,
            told.1
        ));
        // Read again: the open list shows the client's answers.
        click_last(&mut h, "Read again");
        assert!(wait_for(&mut h, |h| h
            .query_by_label_contains("Sample W-2 2026.pdf")
            .is_some()));
        h.run_steps(6);
        shot(&mut h, "10-vf-answers-with-who-when-ip");

        // ---- Step 9: the client's document comes down to the Portal inbox; the report too.
        open_mut(&mut h).browse.portal.fetch_now = true;
        let inbox = vf_portal::inbox::inbox(&root);
        let start = std::time::Instant::now();
        let has = |part: &str| {
            std::fs::read_dir(&inbox)
                .map(|d| {
                    d.flatten()
                        .any(|e| e.file_name().to_string_lossy().contains(part))
                })
                .unwrap_or(false)
        };
        while !has("Sample W-2 2026") && start.elapsed().as_secs() < 60 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            h.step();
        }
        assert!(
            has("Sample W-2 2026"),
            "the upload reaches the Portal inbox"
        );
        click_last(&mut h, "File report in the client folder");
        assert!(wait_for(&mut h, |h| said(
            h,
            "The report is in the Portal inbox"
        )));
        h.run_steps(4);
        shot(&mut h, "11-vf-report-filed");
        let files: Vec<String> = std::fs::read_dir(&inbox)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let hints = std::fs::read_to_string(root.join("_Vibe").join("ingest_hints.jsonl"))
            .unwrap_or_default();
        note(format!(
            "Step 9: Portal inbox now holds {files:?}; Intake hint for the upload: {}",
            hints.lines().last().unwrap_or("none")
        ));

        // ---- Step 10: cancel the answered list; a second, unanswered one is deleted.
        click_last(&mut h, "Cancel list");
        h.run_steps(4);
        shot(&mut h, "12-vf-cancel-confirm");
        click_last(&mut h, "Cancel list");
        assert!(wait_for(&mut h, |h| said(h, "Cancelled.")));
        // Cancelled is final (owner 2026-11-23): not opened again.
        assert!(
            h.query_by_role_and_label(Role::Button, "Open again")
                .is_none()
        );
        h.run_steps(4);
        shot(&mut h, "13-vf-cancelled");
        click_last(&mut h, "Hide");
        click_last(&mut h, "From last year's return…");
        {
            use egui_kittest::kittest::NodeT;
            h.query_all_by_role(Role::RadioButton)
                .find(|n| {
                    n.accesskit_node()
                        .label()
                        .is_some_and(|l| l.ends_with("2025 1040 Tax Return.pdf"))
                })
                .unwrap()
                .click();
        }
        h.run_steps(4);
        click_last(&mut h, "Read it");
        assert!(wait_for(&mut h, |h| h
            .query_by_label("Items read from last year's return")
            .is_some()));
        click_last(&mut h, &use_label);
        click_last(&mut h, "Send the list");
        assert!(wait_for(&mut h, |h| h
            .query_by_label_contains("1. W-2 from")
            .is_some()));
        h.run_steps(4);
        click_last(&mut h, "Delete");
        h.run_steps(4);
        click_last(&mut h, "Delete");
        assert!(wait_for(&mut h, |h| said(h, "The list was deleted.")));
        h.run_steps(4);
        shot(&mut h, "14-vf-deleted");
        note(
            "Step 10: the answered list cancelled; a second unanswered list sent and deleted"
                .into(),
        );
        assert!(client("after"), "the client's view after");
        std::fs::write(
            out.join("vf-steps.json"),
            serde_json::to_string_pretty(&steps).unwrap(),
        )
        .unwrap();
    }

    /// PA-113 (owner 2026-11-25) walkthrough against a real Vibe Portal: the organizer app's
    /// export (made up here) imported in Admin › Portal › Organizers, matched by UltraTax ID, by
    /// email and name, and by name alone (ticked by hand), a link on another site refused; sent;
    /// the client panel's line; Turn off from the panel; the card gone once the tax return
    /// delivery for that year is created. Set `VF_WALK_URL`, `VF_WALK_KEY`, `VF_WALK_LOG`, `VF_WALK_CLIENT`
    /// (scripts/organizer-walkthrough/client.mjs) and `VF_WALK_DIR`.
    #[test]
    #[ignore = "needs a running portal"]
    fn organizer_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 4).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_deliveries(true);
        vf_portal::settings::save(&root, &s).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 1000.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_by_label_contains(text).is_some()
        };
        let wait_long =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>,
             what: &str,
             ok: &dyn Fn(&egui_kittest::Harness<'static, VibeFilesApp>) -> bool| {
                let start = std::time::Instant::now();
                while !ok(h) {
                    if start.elapsed().as_secs() >= 60 {
                        let _ = h.render().map(|i| i.save(format!("{what}-FAILED.png")));
                        panic!("{what}: never");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    h.step();
                }
            };
        let type_in =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, text: &str| {
                h.get_by_label(label).click();
                h.step();
                h.get_by_label(label).type_text(text);
                h.step();
            };
        // The client's side, in Edge: `expect` is [(client name, card, link)].
        let client = |phase: &str, email: &str, mobile: bool, expect: &[(&str, &str, &str)]| {
            let file = t.path().join(format!("expect-{phase}.json"));
            let list: Vec<serde_json::Value> = expect
                .iter()
                .map(|(c, card, link)| {
                    let mut v = serde_json::json!({ "client": c, "card": card });
                    if !link.is_empty() {
                        v["link"] = serde_json::json!(link);
                    }
                    v
                })
                .collect();
            std::fs::write(&file, serde_json::to_string(&list).unwrap()).unwrap();
            let o = std::process::Command::new("node")
                .arg(&client_js)
                .args([
                    url.as_str(),
                    &out.display().to_string(),
                    log.as_str(),
                    email,
                    phase,
                ])
                .arg(&file)
                .env("VF_MOBILE", if mobile { "1" } else { "0" })
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            assert!(
                o.status.success(),
                "client {phase}: {text}{}",
                String::from_utf8_lossy(&o.stderr)
            );
            text.trim().to_string()
        };
        h.get_by_role_and_label(Role::Button, "Open existing")
            .click();
        h.run_steps(8);
        type_in(&mut h, "Cabinet folder", &root.display().to_string());
        h.get_by_role_and_label(Role::Button, "Open cabinet")
            .click();
        assert!(wait_for(&mut h, |h| matches!(
            h.state().screen,
            crate::app::Screen::Open(_)
        )));
        h.run_steps(8);
        let clients: Vec<(String, String, String)> = open_mut(&mut h)
            .browse
            .snap
            .clients
            .iter()
            .take(4)
            .map(|c| (c.id.clone(), c.ext_id.clone(), c.name.clone()))
            .collect();
        assert_eq!(clients.len(), 4);
        let email = format!("organizer+{run}@example.com");
        let person = |who: &str| format!("{who}+{run}@example.com");

        // ---- 0. The client CSV with one email (Portal contacts), through Import clients CSV….
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let csv = t.path().join("clients.csv");
        let mut text = String::from("Client ID,Client Name,TP First Name,TP Last Name,TP Email\n");
        for (i, c) in clients.iter().enumerate() {
            let e = if i == 1 { email.as_str() } else { "" };
            text.push_str(&format!("{},{},,,{e}\n", c.1, q(&c.2)));
        }
        std::fs::write(&csv, text).unwrap();
        h.get_by_role_and_label(Role::Button, "Import clients CSV…")
            .click();
        h.run_steps(4);
        type_in(&mut h, "CSV file", &csv.display().to_string());
        h.get_by_role_and_label(Role::Button, "Read").click();
        assert!(wait_for(&mut h, |h| said(h, "Changes in this CSV")));
        let import = h
            .query_all_by_role(Role::Button)
            .filter_map(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .filter(|l| l.starts_with("Import ") && l.ends_with("(one transaction)"))
            })
            .next()
            .expect("the Import button");
        h.get_by_role_and_label(Role::Button, &import).click();
        assert!(wait_for(&mut h, |h| said(h, "Portal contacts changed")));

        // ---- 1. The organizer app's export (synthetic), with what goes wrong in real ones.
        let link = |n: &str| format!("https://clients.stanfordtax.com/walk-{run}/organizer/{n}");
        let export = t.path().join("organizer-links.csv");
        let mut x = String::from(
            "ID,Emails,Client,Tax year,Return type,Questionnaire status,Questionnaire link,Source,Source id,Tax return delivery status\n",
        );
        // Line 2: by UltraTax ID.
        x.push_str(&format!(
            "a1,,{},2025,1040,In progress,{},ultratax,{},Not delivered\n",
            q(&clients[0].2),
            link("a1"),
            clients[0].1
        ));
        // Line 3: by email (written "Name <email>") and name.
        x.push_str(&format!(
            "a2,\"Pat <{}>\",{},2025,1120s,Not seen yet,{},,,Not delivered\n",
            email.to_uppercase(),
            q(&clients[1].2),
            link("a2")
        ));
        // Line 4: name only.
        x.push_str(&format!(
            "a3,,{},2025,1065,Submitted,{},,,Not delivered\n",
            q(&clients[2].2),
            link("a3")
        ));
        // Line 5: a link on another site.
        x.push_str(
            "a4,,\"Nobody, Here\",2025,1040,In progress,https://example.com/organizer/a4,,,Not delivered\n",
        );
        // Line 6: the ID is client 3, the email client 1's: asks.
        x.push_str(&format!(
            "a5,{email},{},2025,1040,In progress,{},ultratax,{},Not delivered\n",
            q(&clients[3].2),
            link("a5"),
            clients[3].1
        ));
        // Line 7: client 0 a second time.
        x.push_str(&format!(
            "a6,,{},2025,1040,Not seen yet,{},ultratax,{},Not delivered\n",
            q(&clients[0].2),
            link("a6"),
            clients[0].1
        ));
        // Line 8: last year's.
        x.push_str(&format!(
            "a7,,{},2024,1040,Submitted,{},ultratax,{},Delivered\n",
            q(&clients[0].2),
            link("a7"),
            clients[0].1
        ));
        std::fs::write(&export, x).unwrap();
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Organizers").click();
        h.run_steps(6);
        wait_long(&mut h, "the empty page says what to do", &|h| {
            said(h, "None for this tax year")
        });
        let start_import = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>| {
            let ctx = h.ctx.clone();
            open_mut(h).admin.as_mut().unwrap().organizers.start_import(
                &ctx,
                root.clone(),
                export.clone(),
                Vec::new(),
            );
        };
        start_import(&mut h);
        wait_long(&mut h, "the import preview", &|h| said(h, "7 rows"));
        assert!(
            said(
                &h,
                "by ID 2 · by email and name 1 · check 1 · name only 1 · no match 0 · not taken 2"
            ),
            "the preview's counts"
        );
        assert!(
            said(&h, "Two ticked rows go to"),
            "client 0 twice is caught"
        );
        assert!(said(&h, "For 2024, not 2025") && said(&h, "Not an organizer link"));
        assert!(said(&h, "the email is "), "the check row says why");
        shot(&mut h, "01-organizer-import-preview");
        note("Step 1: 7 rows read: 2 by UltraTax ID (one client twice: Send is held until one is unticked), 1 by email written \"Name <EMAIL>\" and name, 1 that asks (ID and email disagree), 1 name only (unticked), a link on another site and last year's row not taken".into());
        {
            let a = open_mut(&mut h).admin.as_mut().unwrap();
            a.organizers.set_import_row(7, false);
            a.organizers.set_import_row(4, true);
            a.organizers.set_import_row(6, true);
        }
        h.run_steps(4);
        assert!(!said(&h, "Two ticked rows go to"));
        h.get_by_role_and_label(Role::Button, "Send 4 links")
            .click();
        wait_long(&mut h, "sent", &|h| said(h, "Sent: 4 new"));
        // Told is ticked to start; nobody has a portal login yet.
        assert!(
            said(
                &h,
                "0 people told their organizer is ready; 4 clients have nobody on the portal yet"
            ),
            "the message says who was told"
        );
        shot(&mut h, "02-organizer-sent");
        note("Step 2: the second row for client 0 unticked, the name-only and the asking rows ticked: Send 4 links, 4 new (clients not on the portal yet added)".into());
        h.get_by_role_and_label(Role::Button, "Close").click();
        h.run_steps(6);
        wait_long(&mut h, "the list", &|h| said(h, "4 listed · 4 showing"));
        shot(&mut h, "03-organizers-page");
        // The same export again changes nothing.
        start_import(&mut h);
        wait_long(&mut h, "the preview again", &|h| said(h, "7 rows"));
        {
            let a = open_mut(&mut h).admin.as_mut().unwrap();
            a.organizers.set_import_row(7, false);
            a.organizers.set_import_row(4, true);
            a.organizers.set_import_row(6, true);
        }
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Send 4 links")
            .click();
        wait_long(&mut h, "sent again", &|h| {
            said(h, "Sent: 0 new, 0 changed, 4 the same.")
        });
        h.get_by_role_and_label(Role::Button, "Close").click();
        h.run_steps(4);
        note("Step 3: the list shows 4, all showing; the same export again: 0 new, 0 changed, 4 the same".into());

        // ---- 4. The clients see their cards (desktop and phone).
        let p = crate::portal::client(&s);
        let (pa, pb) = (person("pat"), person("pat.phone"));
        for e in [&pa, &pb] {
            p.invite(&clients[0].0, Some(e), None, "Pat Sample", "Owner")
                .unwrap();
            p.invite(&clients[2].0, Some(e), None, "Pat Sample", "Owner")
                .unwrap();
        }
        let seen = [
            (clients[0].2.as_str(), "in_progress", link("a1")),
            (clients[2].2.as_str(), "submitted", link("a3")),
        ];
        let want: Vec<(&str, &str, &str)> =
            seen.iter().map(|(c, k, l)| (*c, *k, l.as_str())).collect();
        note(format!(
            "Step 4 (desktop): {}",
            client("see", &pa, false, &want)
        ));
        note(format!(
            "Step 4 (phone): {}",
            client("see", &pb, true, &want)
        ));

        // ---- 5. The client panel's line, and Turn off from it.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
        }
        wait_long(&mut h, "the panel's line", &|h| {
            said(h, "Organizer 2025: In progress · card showing")
        });
        shot(&mut h, "04-panel-organizer");
        h.get_by_role_and_label(Role::Button, "Turn off").click();
        wait_long(&mut h, "turned off", &|h| {
            said(h, "Organizer 2025: In progress · card turned off")
        });
        shot(&mut h, "05-panel-turned-off");
        note("Step 5: the client panel's line; Turn off: \"card turned off\"".into());
        let year = |p: &vf_portal::Portal| p.organizers(Some(2025), None).unwrap();
        let of = |list: &[vf_portal::organizer::Organizer], id: &str| {
            list.iter().find(|o| o.client_id == id).cloned().unwrap()
        };
        assert_eq!(of(&year(&p), &clients[0].0).off.as_deref(), Some("off"));

        // ---- 6. The tax return deliveries for 2025 of clients 1 and 2.
        for i in [1usize, 2] {
            let made = std::process::Command::new("curl")
                .args([
                    "-s",
                    "-X",
                    "POST",
                    "-H",
                    &format!("Authorization: Bearer {key}"),
                    "-H",
                    "Content-Type: application/json",
                    "-d",
                    &serde_json::json!({
                        "clientId": clients[i].0, "taxYear": 2025, "returnType": "1040",
                        "steps": { "return": true },
                    })
                    .to_string(),
                    &format!("{url}/api/firm/v1/deliveries"),
                ])
                .output()
                .unwrap();
            assert!(
                String::from_utf8_lossy(&made.stdout).contains("\"delivery\""),
                "{}",
                String::from_utf8_lossy(&made.stdout)
            );
        }
        let gone = [
            (clients[0].2.as_str(), "none", ""),
            (clients[2].2.as_str(), "none", ""),
        ];
        note(format!(
            "Step 6: deliveries created for clients 1 and 2; a new person at clients 0 and 2 sees: {}",
            {
                let e = person("sam");
                p.invite(&clients[0].0, Some(&e), None, "Sam Sample", "Owner")
                    .unwrap();
                p.invite(&clients[2].0, Some(&e), None, "Sam Sample", "Owner")
                    .unwrap();
                client("gone", &e, false, &gone)
            }
        ));

        // ---- 7. Admin › Portal › Organizers: what is off and why; Turn on ticked.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Organizers").click();
        h.run_steps(4);
        click_read_again(&mut h);
        wait_long(&mut h, "off by delivery", &|h| {
            said(h, "4 listed · 1 showing · 2 off by delivery · 1 turned off")
        });
        shot(&mut h, "06-organizers-after-delivery");
        h.get_by_label("Off").click();
        h.run_steps(4);
        open_mut(&mut h)
            .admin
            .as_mut()
            .unwrap()
            .organizers
            .tick_shown();
        h.run_steps(4);
        h.get_by_role_and_label(Role::Button, "Turn on ticked")
            .click();
        wait_long(&mut h, "turned on", &|h| {
            said(
                h,
                "1 card turned on. The others were not turned off by hand",
            )
        });
        shot(&mut h, "07-organizers-turned-on");
        note("Step 7: the page says 1 showing, 2 off by delivery, 1 turned off; Show Off, tick all, Turn on ticked: 1 turned on, and why the other 2 stay off".into());
        let e = person("robin");
        p.invite(&clients[0].0, Some(&e), None, "Robin Sample", "Owner")
            .unwrap();
        note(format!(
            "Step 8: client 0's card is back: {}",
            client(
                "back",
                &e,
                false,
                &[(clients[0].2.as_str(), "in_progress", &link("a1"))]
            )
        ));
        // ---- 9. A new link where people have access: they are told once, without the link.
        let rows = vf_portal::organizer::read_export(&export).unwrap();
        let r = p
            .put_organizers(2026, &[(clients[2].0.clone(), &rows[2])], true)
            .unwrap();
        let mails: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"msg\":\"notify ("))
            .filter_map(|l| serde_json::from_str(l).ok())
            .filter(|m: &serde_json::Value| {
                m["subject"]
                    .as_str()
                    .is_some_and(|s| s.contains("2026 tax organizer is ready"))
            })
            .collect();
        let to: std::collections::BTreeSet<String> = mails
            .iter()
            .filter_map(|m| m["to"].as_str().map(str::to_string))
            .collect();
        assert_eq!(r.told as usize, to.len(), "{to:?}");
        assert!(to.contains(&pa) && to.contains(&pb) && to.contains(&person("sam")));
        assert!(
            mails
                .iter()
                .all(|m| !m["body"].as_str().unwrap_or("").contains("stanfordtax"))
        );
        let again = p
            .put_organizers(2026, &[(clients[2].0.clone(), &rows[2])], true)
            .unwrap();
        assert_eq!((again.same, again.told), (1, 0));
        note(format!(
            "Step 9: a 2026 link for client 2: {} people told once, by email, with the portal's address and no organizer link; sent again: nobody told",
            r.told
        ));
        std::fs::write(out.join("organizer-steps.txt"), steps.join("\n")).unwrap();
    }

    /// PA-103 walkthrough of the other options, against a real Vibe Portal: Admin › To-do
    /// templates (new, edit, send to several clients, delete), a list from a template with a bad
    /// then good due day, reminders and nobody told, a blank list with items reordered, assigning,
    /// editing every detail, close and open again, Delete then Keep it, the client-folder copies,
    /// the portal out of reach, and to-do lists switched off on the portal and in Vibe Files.
    /// PA-105 and PA-106 walkthrough against a real Vibe Portal: a client CSV with Portal
    /// contacts imported through the dialog (preview, then Import), Invite from client list in
    /// groups of 2 with a pause, a contact typed in the client panel, then an access request
    /// made on the portal by the spouse who was not invited: the footer says it is waiting, and
    /// the request page finds her by the email on file and approves her as Spouse in one click.
    /// Set `VF_WALK_URL`, `VF_WALK_KEY`, `VF_WALK_LOG` (the portal's log, EMAIL_PROVIDER=console),
    /// `VF_WALK_IDPNG` (a small PNG for the photo ID) and `VF_WALK_DIR` (output).
    #[test]
    #[ignore = "needs a running portal"]
    fn contacts_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(png), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_IDPNG"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // New addresses each run: the portal keeps the people of earlier runs.
        let addr = |who: &str| format!("{who}+{run}@example.com");
        let since_ms = (run as i64) * 1000;
        let mail = |to: &str| -> Vec<(i64, String, String)> {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains("\"msg\":\"notify ("))
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|m| m["to"] == to && m["time"].as_i64().is_some_and(|t| t >= since_ms))
                .map(|m| {
                    (
                        m["time"].as_i64().unwrap_or(0),
                        m["subject"].as_str().unwrap_or("").to_string(),
                        m["body"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        };
        let curl = |args: &[&str]| -> serde_json::Value {
            let o = std::process::Command::new("curl")
                .arg("-s")
                .args(args)
                .output()
                .unwrap();
            serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null)
        };
        let firm_get = |path: &str| -> serde_json::Value {
            curl(&[
                "-H",
                &format!("Authorization: Bearer {key}"),
                &format!("{url}/api/firm/v1{path}"),
            ])
        };

        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 4).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0).unwrap();
        vf_portal::settings::save(&root, &s).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 1000.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_by_label_contains(text).is_some()
        };
        let type_in =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, text: &str| {
                h.get_by_label(label).click();
                h.step();
                h.get_by_label(label).type_text(text);
                h.step();
            };
        h.get_by_role_and_label(Role::Button, "Open existing")
            .click();
        h.run_steps(8);
        type_in(&mut h, "Cabinet folder", &root.display().to_string());
        h.get_by_role_and_label(Role::Button, "Open cabinet")
            .click();
        assert!(wait_for(&mut h, |h| matches!(
            h.state().screen,
            crate::app::Screen::Open(_)
        )));
        h.run_steps(8);
        let clients: Vec<(String, String, String)> = open_mut(&mut h)
            .browse
            .snap
            .clients
            .iter()
            .take(3)
            .map(|c| (c.id.clone(), c.ext_id.clone(), c.name.clone()))
            .collect();
        assert_eq!(clients.len(), 3);

        // ---- 1. The client CSV with Portal contacts, through Import clients CSV….
        let csv = t.path().join("clients.csv");
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut text = String::from(
            "Client ID,Client Name,TP First Name,TP Last Name,TP Email,TP Cell,SP First Name,SP Email\n",
        );
        text.push_str(&format!(
            "{},{},John,Sample,{},555-201-0001,Jane,{}\n",
            clients[0].1,
            q(&clients[0].2),
            addr("john"),
            addr("jane")
        ));
        text.push_str(&format!(
            "{},{},Luis,Rivera,{},,Ana,{}\n",
            clients[1].1,
            q(&clients[1].2),
            addr("rivera"),
            addr("rivera")
        ));
        text.push_str(&format!(
            "{},{},Morgan,Taylor,{},,,\n",
            clients[2].1,
            q(&clients[2].2),
            addr("morgan")
        ));
        std::fs::write(&csv, text).unwrap();
        h.get_by_role_and_label(Role::Button, "Import clients CSV…")
            .click();
        h.run_steps(4);
        type_in(&mut h, "CSV file", &csv.display().to_string());
        h.get_by_role_and_label(Role::Button, "Read").click();
        assert!(wait_for(&mut h, |h| said(h, "Changes in this CSV")));
        shot(&mut h, "01-import-preview");
        let import = h
            .query_all_by_role(Role::Button)
            .filter_map(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .filter(|l| l.starts_with("Import ") && l.ends_with("(one transaction)"))
            })
            .next()
            .expect("the Import button");
        h.get_by_role_and_label(Role::Button, &import).click();
        assert!(wait_for(&mut h, |h| said(
            h,
            "Portal contacts changed for 3 clients"
        )));
        let (dir, none) = vf_fs::contacts::read(&root).unwrap();
        assert_eq!(dir.len(), 3, "{none} without");

        // ---- 2. Admin › Portal › Invite from client list, groups of 2, a 3 s pause.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Invite from client list")
            .click();
        if !wait_for(&mut h, |h| said(h, "Invite 4 people at 3 clients")) {
            shot(&mut h, "02-invite-list-failed");
            panic!("the invite list did not offer 4 people at 3 clients");
        }
        {
            let a = open_mut(&mut h).admin.as_mut().unwrap();
            a.invite_list.group = 2;
            a.invite_list.pause_secs = 3;
        }
        h.run_steps(4);
        shot(&mut h, "02-invite-list");
        // Jane is left for her own request (step 3).
        let jane_row = h
            .query_all_by_role(Role::CheckBox)
            .filter_map(|n| {
                use egui_kittest::kittest::NodeT;
                n.accesskit_node()
                    .label()
                    .filter(|l| l.starts_with("Jane Sample"))
            })
            .next()
            .expect("Jane's row");
        h.get_by_label(&jane_row).click();
        h.run_steps(4);
        assert!(said(&h, "Invite 3 people at 3 clients"));
        h.get_by_role_and_label(Role::Button, "Invite 3 people at 3 clients")
            .click();
        assert!(wait_for(&mut h, |h| said(h, "next group in")));
        shot(&mut h, "03-invite-pausing");
        assert!(wait_for(&mut h, |h| said(
            h,
            "3 invited (3 by email, 0 by text)"
        )));
        shot(&mut h, "04-invite-done");
        let at = |to: &str| -> i64 {
            mail(to)
                .iter()
                .find(|m| m.1.contains("invites you to its client portal"))
                .map(|m| m.0)
                .unwrap_or_else(|| panic!("no invitation to {to}"))
        };
        let (j, r, m) = (at(&addr("john")), at(&addr("rivera")), at(&addr("morgan")));
        assert!(
            m - r.max(j) >= 2500,
            "the third waited for the pause: {j} {r} {m}"
        );
        assert!(mail(&addr("jane")).is_empty(), "Jane was not invited");
        // The shared email is one invitation in both names.
        let c = firm_get("/contacts");
        let inv = c["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == clients[1].0.as_str())
            .unwrap()["invitations"]
            .clone();
        assert_eq!(inv[0]["name"], "Luis & Ana Rivera");
        assert_eq!(inv[0]["roleLabel"], "Taxpayer & Spouse");

        // ---- 3. Jane asks for access on the portal (her email is on file as the spouse's).
        let jane = addr("jane");
        let made = curl(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &serde_json::json!({ "name": "Jane Sample", "email": jane, "clientAs": "Sample" })
                .to_string(),
            &format!("{url}/api/portal/access-requests"),
        ]);
        let rid = made["requestId"]
            .as_str()
            .expect("request made")
            .to_string();
        curl(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/octet-stream",
            "--data-binary",
            &format!("@{png}"),
            &format!("{url}/api/portal/access-requests/{rid}/id-photo?side=front"),
        ]);
        let code = mail(&jane)
            .iter()
            .rev()
            .find_map(|m| {
                m.2.split(|c: char| !c.is_ascii_digit())
                    .find(|w| w.len() == 6)
                    .map(String::from)
            })
            .expect("a code to Jane");
        let ok = curl(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &serde_json::json!({ "code": code, "noBarcode": true }).to_string(),
            &format!("{url}/api/portal/access-requests/{rid}/confirm"),
        ]);
        assert!(ok.get("error").is_none(), "confirmed: {ok}");

        // ---- 4. The footer says it is waiting; a click opens Portal requests.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        open_mut(&mut h).browse.portal.look_at_requests_now();
        assert!(wait_for(&mut h, |h| said(
            h,
            "1 portal access request waiting"
        )));
        shot(&mut h, "05-footer-request-waiting");
        h.get_by_label("1 portal access request waiting").click();
        assert!(wait_for(&mut h, |h| said(
            h,
            "The email they confirmed is on file for:"
        )));
        assert!(said(
            &h,
            &format!(
                "{} ({}) as Spouse: Jane Sample's email on file",
                clients[0].2, clients[0].1
            )
        ));
        shot(&mut h, "06-request-matched");
        h.get_by_role_and_label(Role::Button, "Approve as Spouse")
            .click();
        assert!(wait_for(&mut h, |h| said(h, "Approved.")));
        shot(&mut h, "07-request-approved");
        let users = firm_get(&format!("/clients/{}/users", clients[0].0));
        let her = users["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["email"] == jane.as_str())
            .expect("Jane can sign in");
        assert_eq!(her["roleLabel"], "Spouse");
    }

    /// PA-105/PA-106 options walkthrough against a real Vibe Portal (owner 2026-11-24: "testing
    /// various actions to ensure no process gaps or code errors"). Synthetic people only.
    /// A. Import a client CSV with contacts (Jane is the spouse at one client and the contact of
    ///    an LLC; Alex Lead is on file by name only). B. The footer's waiting request opens
    ///    Portal requests although Admin was never opened. C. Jane's request: both clients, a
    ///    role at each, a document shared on Approve. D. Alex's request: a name lead, How did
    ///    you check?, the confirmed email kept. E. A stranger's request: Deny. F. A contact typed
    ///    in the client panel, then a CSV that changes it: the preview says it is kept. G. Invite
    ///    from client list: Search, Stop during the pause, Read again, the rest sent, nobody
    ///    twice. H. Share on portal: someone with a login added at once. I. To-do notices by
    ///    text too. Set `VF_WALK_URL`, `VF_WALK_KEY`, `VF_WALK_LOG`, `VF_WALK_IDPNG`,
    ///    `VF_WALK_PG` (the portal's Postgres container) and `VF_WALK_DIR`.
    #[test]
    #[ignore = "needs a running portal"]
    fn contacts_options_walkthrough() {
        let (Ok(url), Ok(key), Ok(log), Ok(png), Ok(pg), Ok(out)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_IDPNG"),
            std::env::var("VF_WALK_PG"),
            std::env::var("VF_WALK_DIR"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let addr = |who: &str| format!("{who}+{run}@example.com");
        let since_ms = (run as i64) * 1000;
        let mail = |to: &str| -> Vec<(i64, String, String)> {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains("\"msg\":\"notify ("))
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|m| m["to"] == to && m["time"].as_i64().is_some_and(|t| t >= since_ms))
                .map(|m| {
                    (
                        m["time"].as_i64().unwrap_or(0),
                        m["subject"].as_str().unwrap_or("").to_string(),
                        m["body"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        };
        // (The portal logs a message just after it answers: looked at for a few seconds.)
        let invitations_to = |to: &str| {
            let n = || {
                mail(to)
                    .iter()
                    .filter(|m| m.1.contains("invites you to its client portal"))
                    .count()
            };
            let start = std::time::Instant::now();
            while n() == 0 && start.elapsed().as_secs() < 3 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            n()
        };
        let curl = |args: &[&str]| -> serde_json::Value {
            let o = std::process::Command::new("curl")
                .arg("-s")
                .args(args)
                .output()
                .unwrap();
            serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null)
        };
        let firm = |method: &str, path: &str, body: Option<serde_json::Value>| {
            let auth = format!("Authorization: Bearer {key}");
            let u = format!("{url}/api/firm/v1{path}");
            let b = body.map(|b| b.to_string());
            let mut args = vec![
                "-X",
                method,
                "-H",
                &auth,
                "-H",
                "Content-Type: application/json",
            ];
            if let Some(b) = &b {
                args.push("-d");
                args.push(b);
            }
            args.push(&u);
            curl(&args)
        };
        let psql = |sql: &str| {
            let ok = std::process::Command::new("docker")
                .args(["exec", &pg, "psql", "-U", "vibe", "-d", "walk", "-c", sql])
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "psql {sql}");
        };
        // A person asks for access on the portal and confirms their email.
        let ask = |name: &str, email: &str| {
            let made = curl(&[
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "-d",
                &serde_json::json!({ "name": name, "email": email, "clientAs": "Walkthrough" })
                    .to_string(),
                &format!("{url}/api/portal/access-requests"),
            ]);
            let rid = made["requestId"]
                .as_str()
                .expect("request made")
                .to_string();
            curl(&[
                "-X",
                "POST",
                "-H",
                "Content-Type: application/octet-stream",
                "--data-binary",
                &format!("@{png}"),
                &format!("{url}/api/portal/access-requests/{rid}/id-photo?side=front"),
            ]);
            let code = mail(email)
                .iter()
                .rev()
                .find_map(|m| {
                    m.2.split(|c: char| !c.is_ascii_digit())
                        .find(|w| w.len() == 6)
                        .map(String::from)
                })
                .expect("a code");
            let ok = curl(&[
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "-d",
                &serde_json::json!({ "code": code, "noBarcode": true }).to_string(),
                &format!("{url}/api/portal/access-requests/{rid}/confirm"),
            ]);
            assert!(ok.get("error").is_none(), "confirmed: {ok}");
            rid
        };

        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 5).unwrap();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0).unwrap();
        vf_portal::settings::save(&root, &s).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 1000.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_by_label_contains(text).is_some()
        };
        let type_in =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str, text: &str| {
                h.get_by_label(label).click();
                h.step();
                h.get_by_label(label).type_text(text);
                h.step();
            };
        let wait_said =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, text: &str, name: &str| {
                if !wait_for(h, |h| h.query_by_label_contains(text).is_some()) {
                    for _ in 0..5 {
                        h.step();
                    }
                    if let Ok(img) = h.render() {
                        img.save(out.join(format!("{name}-failed.png"))).unwrap();
                    }
                    panic!("{name}: never showed {text:?}");
                }
            };
        let button_starting = |h: &egui_kittest::Harness<'static, VibeFilesApp>, start: &str| {
            h.query_all_by_role(Role::Button)
                .filter_map(|n| {
                    use egui_kittest::kittest::NodeT;
                    n.accesskit_node()
                        .label()
                        .filter(|l| l.starts_with(start) && l.contains(" at "))
                })
                .next()
                .unwrap_or_else(|| panic!("no button starting {start:?}"))
        };
        let checkbox_starting = |h: &egui_kittest::Harness<'static, VibeFilesApp>, start: &str| {
            h.query_all_by_role(Role::CheckBox)
                .filter_map(|n| {
                    use egui_kittest::kittest::NodeT;
                    n.accesskit_node().label().filter(|l| l.starts_with(start))
                })
                .next()
                .unwrap_or_else(|| panic!("no checkbox starting {start:?}"))
        };
        h.get_by_role_and_label(Role::Button, "Open existing")
            .click();
        h.run_steps(8);
        type_in(&mut h, "Cabinet folder", &root.display().to_string());
        h.get_by_role_and_label(Role::Button, "Open cabinet")
            .click();
        assert!(wait_for(&mut h, |h| matches!(
            h.state().screen,
            crate::app::Screen::Open(_)
        )));
        h.run_steps(8);
        let clients: Vec<(String, String, String)> = open_mut(&mut h)
            .browse
            .snap
            .clients
            .iter()
            .map(|c| (c.id.clone(), c.ext_id.clone(), c.name.clone()))
            .collect();
        assert_eq!(clients.len(), 5);
        let (c0, c1, c2, c3) = (&clients[0], &clients[1], &clients[2], &clients[3]);
        // The second client is an LLC.
        vf_fs::clients::set_entity_type(&root, &c1.0, Some("llc")).unwrap();

        // ---- A. The client CSV with Portal contacts.
        let csv = t.path().join("clients.csv");
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let write_csv = |morgan: &str| {
            let mut text = String::from(
                "Client ID,Client Name,TP First Name,TP Last Name,TP Email,TP Cell,SP First Name,SP Email\n",
            );
            text.push_str(&format!(
                "{},{},John,Sample,{},555-201-0001,Jane,{}\n",
                c0.1,
                q(&c0.2),
                addr("john"),
                addr("jane")
            ));
            text.push_str(&format!(
                "{},{},Jane,Sample,{},,,\n",
                c1.1,
                q(&c1.2),
                addr("jane")
            ));
            text.push_str(&format!(
                "{},{},Morgan,Taylor,{morgan},,,\n",
                c2.1,
                q(&c2.2)
            ));
            text.push_str(&format!("{},{},Alex,Lead,,,,\n", c3.1, q(&c3.2)));
            std::fs::write(&csv, text).unwrap();
        };
        write_csv(&addr("morgan"));
        let import =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, check: &str, name: &str| {
                h.get_by_role_and_label(Role::Button, "Browse").click();
                h.run_steps(4);
                h.get_by_role_and_label(Role::Button, "Import clients CSV…")
                    .click();
                h.run_steps(4);
                h.get_by_label("CSV file").click();
                h.step();
                h.get_by_label("CSV file")
                    .type_text(&csv.display().to_string());
                h.step();
                h.get_by_role_and_label(Role::Button, "Read").click();
                if !wait_for(h, |h| h.query_by_label_contains(check).is_some()) {
                    for _ in 0..5 {
                        h.step();
                    }
                    if let Ok(img) = h.render() {
                        img.save(out.join(format!("{name}-failed.png"))).unwrap();
                    }
                    panic!("{name}: never showed {check:?}");
                }
                for _ in 0..5 {
                    h.step();
                }
                if let Ok(img) = h.render() {
                    img.save(out.join(format!("{name}.png"))).unwrap();
                }
                let label = h
                    .query_all_by_role(Role::Button)
                    .filter_map(|n| {
                        use egui_kittest::kittest::NodeT;
                        n.accesskit_node().label().filter(|l| {
                            l.starts_with("Import ") && l.ends_with("(one transaction)")
                        })
                    })
                    .next()
                    .expect("the Import button");
                h.get_by_role_and_label(Role::Button, &label).click();
                assert!(wait_for(h, |h| h
                    .query_by_label_contains("Portal contacts changed for")
                    .is_some()));
                h.run_steps(4);
            };
        import(&mut h, "Import changes:", "A1-import-preview");

        // What Approve offers to share: a document of the first client, of a doc type.
        let (doc_id, doc_type): (String, String) = {
            let o = open_mut(&mut h);
            let dt: String = o
                .opened
                .conn
                .query_row(
                    "SELECT id FROM doc_type WHERE deleted_at IS NULL ORDER BY sort, name LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let doc: String = o
                .opened
                .conn
                .query_row(
                    "SELECT id FROM document WHERE client_id = ?1 AND deleted_at IS NULL ORDER BY rel_path LIMIT 1",
                    [&c0.0],
                    |r| r.get(0),
                )
                .unwrap();
            o.opened
                .conn
                .execute(
                    "UPDATE document SET doc_type_id = ?1, tax_year = 2025 WHERE id = ?2",
                    [&dt, &doc],
                )
                .unwrap();
            (doc, dt)
        };
        let mut s2 = vf_portal::settings::load(&root, &cab).unwrap();
        s2.approve_share = vf_portal::settings::ApproveShare {
            doc_types: vec![doc_type.clone()],
            years: 1,
        };
        vf_portal::settings::save(&root, &s2).unwrap();
        // ---- B. Jane asks; the footer says so; Admin was never opened.
        let jane = addr("jane");
        ask("Jane Sample", &jane);
        open_mut(&mut h).browse.portal.look_at_requests_now();
        wait_said(&mut h, "1 portal access request waiting", "B1-footer");
        assert!(open_mut(&mut h).admin.is_none(), "Admin not opened yet");
        h.get_by_label("1 portal access request waiting").click();
        wait_said(
            &mut h,
            "The email they confirmed is on file for:",
            "B2-requests-page",
        );
        shot(&mut h, "B2-requests-page");

        // ---- C. Both clients, a role at each, a document shared on Approve.
        wait_said(&mut h, "Also share with them:", "C1-share-offered");
        assert!(said(&h, &format!("{} ({}) as Spouse", c0.2, c0.1)));
        assert!(said(&h, &format!("{} ({}) as Owner", c1.2, c1.1)));
        shot(&mut h, "C1-two-clients");
        h.get_by_role_and_label(Role::Button, "Approve for 2 clients")
            .click();
        wait_said(&mut h, "Approved for 2 clients.", "C2-approved");
        wait_said(&mut h, "1 document shared with them.", "C2-shared");
        shot(&mut h, "C2-approved");
        let role_of = |client: &str, email: &str| -> String {
            let u = firm("GET", &format!("/clients/{client}/users"), None);
            u["users"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["email"] == email)
                .map(|x| x["roleLabel"].as_str().unwrap_or("").to_string())
                .unwrap_or_default()
        };
        assert_eq!(role_of(&c0.0, &jane), "Spouse");
        assert_eq!(role_of(&c1.0, &jane), "Owner");
        let files = firm("GET", &format!("/files?clientId={}", c0.0), None);
        let shared = files["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["vfDocumentId"] == doc_id.as_str())
            .expect("the document is on the portal");
        assert!(
            shared["users"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u["visible"] == true)
        );

        // ---- D. A name lead: How did you check?, and the email kept.
        let alex = addr("alex");
        ask("Alex Lead", &alex);
        h.get_by_role_and_label(Role::Button, "Refresh").click();
        wait_said(
            &mut h,
            "Not a match: the email they confirmed is not on file.",
            "D1-lead",
        );
        assert!(said(
            &h,
            &format!("{} ({}) as Taxpayer: Alex Lead", c3.2, c3.1)
        ));
        shot(&mut h, "D1-lead");
        h.get_by_role_and_label(Role::Button, "Approve as Taxpayer…")
            .click();
        wait_said(&mut h, "How did you check Alex Lead?", "D2-ask");
        h.get_by_label("Called the number in our records").click();
        h.run_steps(2);
        type_in(&mut h, "Who you spoke to", "Alex Lead");
        h.get_by_role_and_label(Role::Button, "Approve with this check")
            .click();
        wait_said(
            &mut h,
            "Their contact is kept in the client's Portal contacts.",
            "D3-approved",
        );
        assert_eq!(role_of(&c3.0, &alex), "Taxpayer");
        let rec = vf_fs::contacts::record(&root, &c3.0).unwrap();
        assert_eq!(
            rec.hand.get("email").map(String::as_str),
            Some(alex.as_str())
        );

        // ---- E. Nobody's request: Deny.
        let stranger = addr("stranger");
        ask("Chris Nobody", &stranger);
        h.get_by_role_and_label(Role::Button, "Refresh").click();
        wait_said(&mut h, "Chris Nobody", "E1-stranger");
        shot(&mut h, "E1-stranger");
        h.get_by_role_and_label(Role::Button, "Deny").click();
        wait_said(&mut h, "No requests waiting.", "E2-denied");
        let left = firm("GET", "/access-requests", None);
        assert_eq!(left["requests"].as_array().map(Vec::len), Some(0));

        // ---- F. A contact typed in the panel; a CSV that changes it keeps the typed one.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(4);
        {
            let o = open_mut(&mut h);
            let i = o
                .browse
                .snap
                .clients
                .iter()
                .position(|c| c.id == c2.0)
                .unwrap();
            o.browse.set_scope(vf_core::browse::Scope::Client(i));
        }
        h.run_steps(6);
        h.get_by_label("PORTAL CONTACTS").click();
        h.run_steps(4);
        wait_said(
            &mut h,
            &format!("Taxpayer: Morgan Taylor · {}", addr("morgan")),
            "F1-panel",
        );
        h.get_by_role_and_label(Role::Button, "Edit contacts…")
            .click();
        h.run_steps(4);
        // The email field holds the CSV's: replaced by the typed one.
        let typed = addr("morgan.typed");
        {
            let o = open_mut(&mut h);
            // (Typed through the field's state: select-all is not reliable in the harness.)
            if let Some(bx) = o.browse.contacts_box.as_mut() {
                bx.set_editing_email(&typed);
            }
        }
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Save contacts")
            .click();
        wait_said(&mut h, &format!("{typed} (typed)"), "F2-typed");
        shot(&mut h, "F2-typed");
        write_csv(&addr("morgan.csv"));
        import(
            &mut h,
            "(set by hand: the typed value is kept)",
            "F3-preview-kept",
        );
        let rec = vf_fs::contacts::record(&root, &c2.0).unwrap();
        assert_eq!(rec.effective().email, typed);
        assert_eq!(rec.csv.email, addr("morgan.csv"));

        // ---- G. Invite from client list: Search, Stop, Read again, the rest.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "Invite from client list")
            .click();
        wait_said(&mut h, "with contacts;", "G1-list");
        {
            let a = open_mut(&mut h).admin.as_mut().unwrap();
            a.invite_list.group = 1;
            a.invite_list.pause_secs = 4;
        }
        // Jane and Alex are on the portal: their clients are listed under Every client only.
        h.get_by_label("Show").click();
        h.run_steps(2);
        h.get_by_label("Every client").click();
        h.run_steps(4);
        type_in(&mut h, "Search", "morgan");
        wait_said(&mut h, "Invite 1 person at 1 client", "G2-search");
        shot(&mut h, "G2-search");
        h.get_by_role_and_label(Role::Button, "Clear").click();
        h.run_steps(4);
        // John's client has Jane on the portal: John is not ticked until staff say he is new.
        wait_said(
            &mut h,
            "tick only if John Sample is someone new",
            "G3-check",
        );
        let john_box = checkbox_starting(&h, "John Sample");
        h.get_by_label(&john_box).click();
        h.run_steps(4);
        let first = button_starting(&h, "Invite ");
        shot(&mut h, "G3-list");
        assert_eq!(first, "Invite 2 people at 2 clients", "John and Morgan");
        h.get_by_role_and_label(Role::Button, &first).click();
        wait_said(&mut h, "next group in", "G4-pausing");
        h.get_by_role_and_label(Role::Button, "Stop").click();
        wait_said(&mut h, "left when stopped", "G5-stopped");
        shot(&mut h, "G5-stopped");
        let john = addr("john");
        let morgan_typed = typed.clone();
        let sent_first = invitations_to(&john) + invitations_to(&morgan_typed);
        assert_eq!(sent_first, 1, "one sent before Stop");
        // Show every client: the one invited says so and is not ticked.
        // (After Stop the page reads the portal again by itself; Read again once it has.)
        wait_said(&mut h, "with contacts;", "G6-reread");
        h.get_by_role_and_label(Role::Button, "Read again").click();
        h.run_steps(2);
        // Read again: the list is read anew (the stopped run's words are gone).
        wait_said(&mut h, "with contacts;", "G6-read");
        wait_said(&mut h, "invited, not accepted yet", "G6-waiting");
        shot(&mut h, "G6-waiting");
        let rest = button_starting(&h, "Invite ");
        assert_eq!(rest, "Invite 1 person at 1 client");
        h.get_by_role_and_label(Role::Button, &rest).click();
        wait_said(&mut h, "1 invited (1 by email, 0 by text)", "G7-rest");
        shot(&mut h, "G7-rest");
        assert_eq!(invitations_to(&john), 1, "John once");
        assert_eq!(
            invitations_to(&morgan_typed),
            1,
            "Morgan once, at the typed address"
        );
        assert_eq!(invitations_to(&addr("morgan.csv")), 0, "never the CSV's");
        shot(&mut h, "G7-rest");

        // ---- H. John accepts; then Share on portal adds him to Morgan's client at once.
        let token = mail(&john)
            .iter()
            .find_map(|m| {
                m.2.split("#t=")
                    .nth(1)
                    .map(|t| t.split_whitespace().next().unwrap_or("").to_string())
            })
            .expect("John's link");
        let acc = curl(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &serde_json::json!({ "token": token }).to_string(),
            &format!("{url}/api/portal/invitations/accept"),
        ]);
        assert!(acc.get("error").is_none(), "accepted: {acc}");
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(4);
        {
            let o = open_mut(&mut h);
            let i = o
                .browse
                .snap
                .clients
                .iter()
                .position(|c| c.id == c2.0)
                .unwrap();
            o.browse.set_scope(vf_core::browse::Scope::Client(i));
        }
        h.run_steps(6);
        {
            let o = open_mut(&mut h);
            o.browse.selected.clear();
            let first = o.browse.visible.first().copied().expect("a document");
            o.browse.selected.insert(first);
        }
        h.run_steps(6);
        h.query_all_by_role_and_label(Role::Button, "Share…")
            .next()
            .expect("Share…")
            .click();
        wait_said(&mut h, "Invite someone…", "H1-share");
        h.get_by_label("Invite someone…").click();
        h.run_steps(2);
        type_in(&mut h, "Email", &john);
        wait_said(&mut h, "John Sample already has a portal login", "H2-known");
        shot(&mut h, "H2-known");
        type_in(&mut h, "Role (e.g. Spouse)", "Bookkeeper");
        h.get_by_role_and_label(Role::Button, "Add to this client and share")
            .click();
        wait_said(&mut h, "John Sample was added to this client", "H3-added");
        shot(&mut h, "H3-added");
        assert_eq!(role_of(&c2.0, &john), "Bookkeeper");
        assert_eq!(invitations_to(&john), 1, "no second invitation");
        assert!(
            mail(&john)
                .iter()
                .any(|m| m.2.contains(&format!("has given you access to {}", c2.2))),
            "John was told"
        );

        // ---- I. To-do notices by text too (P114).
        psql("UPDATE firm SET requests_enabled = true, requests_text = true");
        psql(&format!(
            "UPDATE portal_identity SET phone = '+15552010077' WHERE email = '{john}'"
        ));
        let made = firm(
            "POST",
            "/request-lists",
            Some(serde_json::json!({
                "clientIds": [c0.0],
                "title": "2026 tax documents",
                "items": [{ "kind": "text", "prompt": "Any new dependents?", "help": "", "required": true }],
                "notify": true
            })),
        );
        assert!(made.get("lists").is_some(), "list sent: {made}");
        let texts = mail("+15552010077");
        assert!(
            texts
                .iter()
                .any(|m| m.2.contains("2026 tax documents") && !m.2.contains("dependents")),
            "John was texted: {texts:?}"
        );
        assert!(
            mail(&john)
                .iter()
                .any(|m| m.2.contains("2026 tax documents")),
            "and emailed"
        );
    }

    /// Same variables as `todo_walkthrough`, plus `VF_WALK_PG` (the portal's Postgres container,
    /// for the portal's own switch).
    #[test]
    #[ignore = "needs a running portal"]
    fn todo_options_walkthrough() {
        use egui_kittest::kittest::NodeT;
        let (Ok(url), Ok(key), Ok(log), Ok(client_js), Ok(pdf), Ok(out), Ok(pg)) = (
            std::env::var("VF_WALK_URL"),
            std::env::var("VF_WALK_KEY"),
            std::env::var("VF_WALK_LOG"),
            std::env::var("VF_WALK_CLIENT"),
            std::env::var("VF_WALK_PDF"),
            std::env::var("VF_WALK_DIR"),
            std::env::var("VF_WALK_PG"),
        ) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let emails = [
            format!("jordan.sample+{stamp}@example.com"),
            format!("casey.example+{stamp}@example.com"),
        ];
        let since_ms = stamp as i64 * 1000;
        let mut steps: Vec<String> = Vec::new();
        let mut note = |s: String| {
            eprintln!("{s}");
            steps.push(s);
        };
        let accept = |email: &str| -> bool {
            std::process::Command::new("node")
                .args([
                    &client_js,
                    &url,
                    &out.display().to_string(),
                    &log,
                    email,
                    "accept",
                    &pdf,
                ])
                .status()
                .is_ok_and(|s| s.success())
        };
        let mail = |to: &str| -> Vec<(String, String)> {
            std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains("\"msg\":\"notify ("))
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|m| m["to"] == to && m["time"].as_i64().is_some_and(|t| t >= since_ms))
                .map(|m| {
                    (
                        m["subject"].as_str().unwrap_or("").to_string(),
                        m["body"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        };
        let psql = |sql: &str| {
            let ok = std::process::Command::new("docker")
                .args(["exec", &pg, "psql", "-U", "vibe", "-d", "walk", "-c", sql])
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "psql {sql}");
        };

        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Clients");
        vf_fixtures::generate(&root, 5).unwrap();
        crate::ui::test_util::pdfium_first();
        let data = t.path().join("data");
        drop(crate::session::open(&data, "dev", &root).unwrap());
        let cab = vf_fs::cabinet::read(&root).unwrap().cabinet_id;
        let s = vf_portal::PortalSettings::new(&cab, &url, &key, 0)
            .unwrap()
            .with_todo_lists(true);
        vf_portal::settings::save(&root, &s).unwrap();
        let d = data.clone();
        let mut h = HarnessBuilder::default()
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
            .with_theme(egui::Theme::Light)
            .with_size([1440.0, 1000.0])
            .build_eframe(move |cc| VibeFilesApp::new(&cc.egui_ctx, d.clone()));
        h.run_steps(8);
        let shot = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, name: &str| {
            for _ in 0..5 {
                h.step();
            }
            match h.render() {
                Ok(img) => img.save(out.join(format!("{name}.png"))).unwrap(),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };
        let click_last = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, label: &str| {
            let n = h.query_all_by_role_and_label(Role::Button, label).count();
            assert!(n > 0, "no button {label:?}");
            h.query_all_by_role_and_label(Role::Button, label)
                .last()
                .unwrap()
                .scroll_to_me();
            h.run_steps(2);
            h.query_all_by_role_and_label(Role::Button, label)
                .last()
                .unwrap()
                .click();
            h.run_steps(4);
        };
        let said = |h: &egui_kittest::Harness<'static, VibeFilesApp>, text: &str| {
            h.query_all_by_label_contains(text).next().is_some()
        };
        // The text boxes shown, in order: (value, top y).
        let inputs = |h: &egui_kittest::Harness<'static, VibeFilesApp>| -> Vec<(String, f32)> {
            h.query_all_by_role(Role::TextInput)
                .map(|n| (n.value().unwrap_or_default(), n.rect().top()))
                .collect()
        };
        // Types `text` into the `n`th text box, replacing what it held.
        let type_in =
            |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, n: usize, text: &str| {
                h.query_all_by_role(Role::TextInput)
                    .nth(n)
                    .expect("the text box")
                    .focus();
                h.run_steps(2);
                h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
                h.run_steps(1);
                h.query_all_by_role(Role::TextInput)
                    .nth(n)
                    .unwrap()
                    .type_text(text);
                h.run_steps(3);
            };
        // Opens the combo box showing `now` (the `nth` such, from the last) and picks `pick`.
        let pick = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, now: &str, pick: &str| {
            // The choice is what the popup adds (a list row may carry the same words).
            let before: Vec<_> = h
                .query_all_by_label(pick)
                .map(|n| n.accesskit_node().id())
                .collect();
            h.query_all_by_role(Role::ComboBox)
                .rfind(|n| {
                    let a = n.accesskit_node();
                    a.value().as_deref() == Some(now) || a.label().as_deref() == Some(now)
                })
                .unwrap_or_else(|| panic!("no combo showing {now:?}"))
                .click();
            h.run_steps(4);
            h.query_all_by_label(pick)
                .rfind(|n| !before.contains(&n.accesskit_node().id()))
                .unwrap_or_else(|| panic!("no choice {pick:?}"))
                .click();
            h.run_steps(4);
        };
        let p = crate::portal::client(&s);

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
        let clients: Vec<_> = open_mut(&mut h).browse.snap.clients[..2].to_vec();
        for (c, email) in clients.iter().zip(&emails) {
            p.put_client(&c.id, &c.ext_id, &c.name, None).unwrap();
            p.invite(&c.id, Some(email), None, "Sample Person", "Owner")
                .unwrap();
            assert!(accept(email), "{email} accepts the invitation");
        }
        // The lists of an earlier run go first (none has answers).
        for c in &clients {
            for l in p.todo_lists(&c.id).unwrap() {
                p.delete_list(&l.id).unwrap();
            }
        }
        note(format!(
            "Setup: {} and {} each have a person on the portal",
            clients[0].name, clients[1].name
        ));

        // Templates are firm-wide: those of an earlier run go first.
        for t in p.todo_templates().unwrap() {
            p.delete_template(&t.id).unwrap();
        }

        // ---- A. Admin › To-do templates: a new template.
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "Portal").click();
        h.run_steps(8);
        h.get_by_role_and_label(Role::Button, "To-do templates")
            .click();
        assert!(wait_for(&mut h, |h| said(h, "No templates yet.")));
        click_last(&mut h, "+ New template");
        let before = inputs(&h).len();
        // Name, note, the first item's wording.
        type_in(&mut h, before - 3, "Bank loan package");
        type_in(&mut h, before - 2, "For the bank, by the 1st please.");
        type_in(
            &mut h,
            before - 1,
            "Last two years of business bank statements",
        );
        h.get_by_role_and_label(Role::Button, "+ Add item").click();
        h.run_steps(4);
        type_in(&mut h, before, "What is the loan for?");
        pick(&mut h, "Documents", "An answer");
        // The second item is optional.
        h.query_all_by_role(Role::CheckBox).last().unwrap().click();
        h.run_steps(4);
        shot(&mut h, "o01-template-new");
        click_last(&mut h, "Save template");
        assert!(wait_for(&mut h, |h| said(h, "Template saved.")));
        let saved = p.todo_templates().unwrap();
        note(format!(
            "A. Template saved: {:?}",
            saved
                .iter()
                .map(|t| (
                    &t.name,
                    t.items
                        .iter()
                        .map(|i| (&i.kind, &i.prompt, i.required))
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        ));
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].items.len(), 2);
        assert_eq!(saved[0].items[1].kind, "text");
        assert!(!saved[0].items[1].required);

        // ---- B. Edit it: renamed.
        click_last(&mut h, "Edit");
        h.run_steps(4);
        let n = inputs(&h)
            .iter()
            .position(|(v, _)| v == "Bank loan package")
            .expect("the name box");
        type_in(&mut h, n, "Bank loan package 2027");
        click_last(&mut h, "Save template");
        assert!(wait_for(&mut h, |h| said(h, "Template saved.")
            && said(h, "Bank loan package 2027")));
        let saved = p.todo_templates().unwrap();
        assert_eq!(saved[0].name, "Bank loan package 2027");
        assert_eq!(saved.len(), 1, "edited, not added");
        note("B. Template renamed (one template, same items)".into());

        // ---- C. Send it to two clients.
        click_last(&mut h, "Send to clients…");
        h.run_steps(4);
        for c in &clients {
            h.get_by_label(&format!("{}  {}", c.name, c.ext_id)).click();
            h.run_steps(2);
        }
        let due = inputs(&h)
            .iter()
            .rposition(|(v, _)| v.is_empty())
            .expect("the due box");
        type_in(&mut h, due, "2027-02-01");
        shot(&mut h, "o02-template-send");
        click_last(&mut h, "Send to 2 clients");
        assert!(wait_for(&mut h, |h| said(
            h,
            "Sent to 2 clients; 2 people were told."
        )));
        shot(&mut h, "o03-template-sent");
        for (c, email) in clients.iter().zip(&emails) {
            let lists = p.todo_lists(&c.id).unwrap();
            assert!(
                lists.iter().any(|l| l.title == "Bank loan package 2027"
                    && l.due_date.as_deref() == Some("2027-02-01")),
                "{} has the list",
                c.name
            );
            assert!(
                mail(email)
                    .iter()
                    .any(|m| m.1.contains("has sent you a to-do list")),
                "{email} is told"
            );
        }
        // Each client's folder gets its copy, as from the client's own window.
        let copies_of = |h: &mut egui_kittest::Harness<'static, VibeFilesApp>, i: usize| {
            let rel = open_mut(h).browse.snap.clients[i]
                .folder_rel
                .clone()
                .expect("a client folder");
            std::fs::read_dir(root.join(rel).join("_Vibe").join("todo"))
                .map(|d| d.flatten().count())
                .unwrap_or(0)
        };
        h.run_steps(10);
        assert_eq!(
            copies_of(&mut h, 0),
            1,
            "a copy in the first client's folder"
        );
        assert_eq!(
            copies_of(&mut h, 1),
            1,
            "a copy in the second client's folder"
        );
        note("C. Sent to both clients: each has the list (due 2027-02-01), was emailed, and has its copy in its folder".into());

        // ---- E. The client's window: the list sent from Admin shows; a list from the template.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(6);
        {
            let o = open_mut(&mut h);
            o.browse.set_scope(vf_core::browse::Scope::Client(0));
            o.browse.portal.reread();
        }
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "To-do lists…")
            .click();
        assert!(wait_for(&mut h, |h| said(h, "Bank loan package 2027")));
        h.run_steps(4);
        pick(&mut h, "From a template…", "Bank loan package 2027");
        assert!(
            h.query_by_role_and_label(Role::Button, "Send the list")
                .is_some()
        );
        let mails_before = mail(&emails[0]).len();
        let due = inputs(&h)
            .iter()
            .rposition(|(v, _)| v.is_empty())
            .expect("the due box");
        type_in(&mut h, due, "3/15/2027");
        assert!(said(&h, "Write the day as 2027-03-15"));
        assert!(
            h.get_by_role_and_label(Role::Button, "Send the list")
                .accesskit_node()
                .is_disabled(),
            "a bad day cannot be sent"
        );
        shot(&mut h, "o04-bad-due-day");
        type_in(&mut h, due, "2027-03-15");
        assert!(!said(&h, "Write the day as 2027-03-15"));
        pick(&mut h, "Never", "Every 7 days while items are open");
        h.get_by_label("Tell the client's people now (email or text, as each chose)")
            .click();
        h.run_steps(4);
        shot(&mut h, "o05-from-template-filled");
        click_last(&mut h, "Send the list");
        assert!(wait_for(&mut h, |h| said(
            h,
            "The list was sent. Nobody was told (Tell the client"
        )));
        h.run_steps(6);
        shot(&mut h, "o06-from-template-sent");
        let lists = p.todo_lists(&clients[0].id).unwrap();
        let made = lists
            .iter()
            .find(|l| l.due_date.as_deref() == Some("2027-03-15"))
            .expect("the list from the template");
        assert_eq!(made.remind_every_days, 7);
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert_eq!(
            mail(&emails[0]).len(),
            mails_before,
            "nobody is told when unticked"
        );
        note("E. From a template: a bad due day is refused; sent with reminders every 7 days and nobody told (no email)".into());

        // ---- E2. Remind now, past the portal's limit of 3 a day for one list.
        for n in 1..=4 {
            click_last(&mut h, "Remind now");
            assert!(wait_for(&mut h, |h| h
                .query_by_role_and_label(Role::Button, "Remind now")
                .is_some_and(|b| !b.accesskit_node().is_disabled())
                && (said(h, "reminded") || said(h, "Try again tomorrow"))));
            if n < 4 {
                assert!(said(&h, "1 person was reminded."), "reminder {n}");
            }
        }
        assert!(said(
            &h,
            "This list was reminded 3 times today already. Try again tomorrow."
        ));
        shot(&mut h, "o06b-remind-limit");
        note("E2. Remind now three times: each told 1 person; the fourth says \"reminded 3 times today already. Try again tomorrow.\"".into());

        // ---- F. A blank list: two answers, reordered.
        click_last(&mut h, "Hide");
        click_last(&mut h, "+ New list");
        let n = inputs(&h).len();
        // Title, note, due, the first item.
        type_in(&mut h, n - 4, "Questions for 2026");
        type_in(&mut h, n - 1, "Did you buy or sell a home in 2026?");
        pick(&mut h, "Documents", "An answer");
        h.get_by_role_and_label(Role::Button, "+ Add item").click();
        h.run_steps(4);
        type_in(&mut h, n, "Any new dependents?");
        pick(&mut h, "Documents", "An answer");
        // The second goes first.
        h.query_all_by_role_and_label(Role::Button, "↑")
            .last()
            .unwrap()
            .click();
        h.run_steps(4);
        shot(&mut h, "o07-blank-list");
        click_last(&mut h, "Send the list");
        assert!(wait_for(&mut h, |h| said(
            h,
            "The list was sent; 1 person was told."
        )));
        h.run_steps(6);
        let lists = p.todo_lists(&clients[0].id).unwrap();
        let q = lists
            .iter()
            .find(|l| l.title == "Questions for 2026")
            .expect("the blank list");
        let q = p.todo_list(&q.id).unwrap();
        assert_eq!(
            q.items
                .iter()
                .map(|i| i.prompt.as_str())
                .collect::<Vec<_>>(),
            ["Any new dependents?", "Did you buy or sell a home in 2026?"]
        );
        assert!(q.items.iter().all(|i| i.kind == "text"));
        assert!(
            mail(&emails[0])
                .iter()
                .any(|m| m.1.contains("has sent you a to-do list") && m.0.contains("2 items")),
            "told of the 2 items: {:?}",
            mail(&emails[0])
        );
        note("F. Blank list of two answers, reordered, sent and emailed".into());

        // ---- G. Assign it from the open list, then no one.
        pick(&mut h, "No one", "Pat Preparer");
        assert!(wait_for(&mut h, |h| said(
            h,
            "Pat Preparer is told when the client answers."
        )));
        assert!(p.todo_list(&q.id).unwrap().assigned_staff_id.is_some());
        pick(&mut h, "Pat Preparer", "No one");
        assert!(wait_for(&mut h, |h| said(h, "No one is assigned now.")));
        assert!(p.todo_list(&q.id).unwrap().assigned_staff_id.is_none());
        note("G. Assigned to Pat Preparer from the open list, then to no one".into());

        // ---- H. Edit every detail: title, note, due day, reminders, an item added.
        click_last(&mut h, "Edit");
        assert!(wait_for(&mut h, |h| h
            .query_by_role_and_label(Role::Button, "Save")
            .is_some()));
        let at = |h: &egui_kittest::Harness<'static, VibeFilesApp>, v: &str| {
            inputs(h).iter().position(|(x, _)| x == v).unwrap()
        };
        let title = at(&h, "Questions for 2026");
        type_in(&mut h, title, "Questions for your 2026 return");
        type_in(&mut h, title + 1, "Short answers are fine.");
        type_in(&mut h, title + 2, "2027-04-01");
        pick(&mut h, "Never", "Every 3 days while items are open");
        h.query_all_by_role_and_label(Role::Button, "+ Add item")
            .last()
            .unwrap()
            .click();
        h.run_steps(4);
        let n = inputs(&h).len();
        type_in(&mut h, n - 1, "Your bank's routing number for the refund");
        shot(&mut h, "o08-editing-details");
        click_last(&mut h, "Save");
        assert!(wait_for(&mut h, |h| said(h, "Saved.")));
        h.run_steps(4);
        let e = p.todo_list(&q.id).unwrap();
        note(format!(
            "H. Edited: title {:?}, note {:?}, due {:?}, reminders every {} days, {} items",
            e.title,
            e.note,
            e.due_date,
            e.remind_every_days,
            e.items.len()
        ));
        assert_eq!(e.title, "Questions for your 2026 return");
        assert_eq!(e.note, "Short answers are fine.");
        assert_eq!(e.due_date.as_deref(), Some("2027-04-01"));
        assert_eq!(e.remind_every_days, 3);
        assert_eq!(e.items.len(), 3);
        shot(&mut h, "o09-edited");

        // ---- I. Close, then open again.
        click_last(&mut h, "Close");
        assert!(wait_for(&mut h, |h| h
            .query_by_role_and_label(Role::Button, "Open again")
            .is_some()));
        h.run_steps(4);
        shot(&mut h, "o10-closed");
        assert_eq!(p.todo_list(&q.id).unwrap().status, "closed");
        assert!(
            h.query_by_role_and_label(Role::Button, "Remind now")
                .is_none()
        );
        click_last(&mut h, "Open again");
        assert!(wait_for(&mut h, |h| h
            .query_by_role_and_label(Role::Button, "Remind now")
            .is_some()));
        assert_eq!(p.todo_list(&q.id).unwrap().status, "open");
        note("I. Closed (no Remind/Edit offered), then opened again".into());

        // ---- J. Delete, Keep it; then Delete.
        click_last(&mut h, "Delete");
        assert!(said(&h, "Delete this list? Nobody has answered it."));
        click_last(&mut h, "Keep it");
        assert!(!said(&h, "Delete this list?"));
        assert!(p.todo_list(&q.id).is_ok(), "kept");
        click_last(&mut h, "Delete");
        click_last(&mut h, "Delete");
        assert!(wait_for(&mut h, |h| said(h, "The list was deleted.")));
        assert!(p.todo_list(&q.id).is_err(), "gone from the portal");
        note("J. Delete → Keep it kept the list; Delete → Delete removed it".into());

        // ---- K. The copies in the client folder; the audit rows.
        h.run_steps(10);
        let folder = root.join(
            open_mut(&mut h).browse.snap.clients[0]
                .folder_rel
                .clone()
                .expect("a client folder"),
        );
        let copies: Vec<String> = std::fs::read_dir(folder.join("_Vibe").join("todo"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let audit: Vec<String> = {
            let conn = &open_mut(&mut h).opened.conn;
            let mut st = conn
                .prepare("SELECT action FROM audit WHERE action LIKE 'portal_todo%' ORDER BY seq")
                .unwrap();
            st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
        };
        note(format!(
            "K. Copies in the client folder: {copies:?}; audit rows: {audit:?}"
        ));
        assert!(!copies.is_empty(), "a copy of each list read");

        // ---- L. The portal out of reach: the copies are shown.
        open_mut(&mut h).browse.portal.todo = None;
        h.run_steps(4);
        let dead = vf_portal::PortalSettings::new(&cab, "http://127.0.0.1:9", &key, 0)
            .unwrap()
            .with_todo_lists(true);
        vf_portal::settings::save(&root, &dead).unwrap();
        open_mut(&mut h).browse.portal.reread();
        h.run_steps(6);
        h.get_by_role_and_label(Role::Button, "To-do lists…")
            .click();
        assert!(wait_for(&mut h, |h| said(
            h,
            "The portal cannot be reached"
        )));
        h.run_steps(6);
        shot(&mut h, "o11-offline-copies");
        note(format!(
            "L. Portal out of reach: copies shown: {}",
            said(&h, "Bank loan package 2027")
        ));
        assert!(
            said(&h, "As read "),
            "a copy is shown with when it was read"
        );
        assert!(
            !said(&h, "Questions for your 2026 return"),
            "the deleted list is not shown"
        );
        vf_portal::settings::save(&root, &s).unwrap();
        open_mut(&mut h).browse.portal.todo = None;
        open_mut(&mut h).browse.portal.reread();
        h.run_steps(6);

        // ---- M. Switched off on the portal: Vibe Files says so.
        psql("update firm set requests_enabled=false");
        h.get_by_role_and_label(Role::Button, "To-do lists…")
            .click();
        h.run_steps(4);
        assert!(wait_for(&mut h, |h| said(
            h,
            "To-do lists are turned off on the portal"
        )));
        h.run_steps(6);
        shot(&mut h, "o12-portal-switched-off");
        note("M. Portal switch off: Vibe Files says \"To-do lists are turned off on the portal: an admin turns them on in the portal's Settings.\"".into());
        psql("update firm set requests_enabled=true");
        click_last(&mut h, "Try again");
        assert!(wait_for(&mut h, |h| h
            .query_by_role_and_label(Role::Button, "+ New list")
            .is_some()));
        note("M. Once on again, Try again reads the lists".into());
        open_mut(&mut h).browse.portal.todo = None;
        h.run_steps(4);

        // ---- N. Switched off in Vibe Files: no button, the templates page says so.
        vf_portal::settings::save(&root, &s.clone().with_todo_lists(false)).unwrap();
        open_mut(&mut h).browse.portal.reread();
        h.run_steps(8);
        assert!(
            h.query_by_role_and_label(Role::Button, "To-do lists…")
                .is_none(),
            "no To-do lists… button when off"
        );
        h.get_by_role_and_label(Role::Button, "Admin").click();
        h.run_steps(6);
        if let Some(a) = &mut open_mut(&mut h).admin {
            a.todo_templates.reread();
        }
        h.get_by_role_and_label(Role::Button, "To-do templates")
            .click();
        h.run_steps(8);
        assert!(said(&h, "To-do lists are off for this cabinet."));
        shot(&mut h, "o13-vf-switched-off");
        note(
            "N. Off in Vibe Files: no To-do lists… button; the templates page says they are off"
                .into(),
        );

        // ---- D. Back on: the template deleted (Keep, then Delete); lists sent stay.
        vf_portal::settings::save(&root, &s).unwrap();
        if let Some(a) = &mut open_mut(&mut h).admin {
            a.todo_templates.reread();
        }
        h.run_steps(40);
        assert!(wait_for(&mut h, |h| said(h, "Bank loan package 2027")));
        click_last(&mut h, "Delete");
        assert!(said(&h, "Delete it?"));
        click_last(&mut h, "Keep");
        assert!(!said(&h, "Delete it?"));
        click_last(&mut h, "Delete");
        h.query_all_by_role_and_label(Role::Button, "Delete")
            .last()
            .unwrap()
            .click();
        assert!(wait_for(&mut h, |h| said(
            h,
            "Template deleted; lists already sent are kept."
        )));
        shot(&mut h, "o14-template-deleted");
        assert!(p.todo_templates().unwrap().is_empty());
        assert!(
            p.todo_lists(&clients[1].id)
                .unwrap()
                .iter()
                .any(|l| l.title == "Bank loan package 2027")
        );
        note("D. Template deleted after Keep; the lists sent from it stay".into());

        std::fs::write(
            out.join("vf-options.json"),
            serde_json::to_string_pretty(&steps).unwrap(),
        )
        .unwrap();
    }

    /// A portal on this PC answering the to-do calls with invented data; its address.
    fn fake_todo_portal() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        const ITEMS: &str = include_str!("../../vf-portal/tests/data/todo_items.json");
        const LIST: &str = r#""id":"l1","clientId":"c","title":"2026 tax documents","note":"Please send these by March 15. Thank you!","dueDate":"2027-03-15","status":"open","complete":false,"done":3,"total":4,"createdAt":"2027-01-10T16:02:00Z","templateId":"t1","createdByKind":"firm_key","createdBy":"OFFICE-PC\\kim","closedAt":null,"closedBy":null,"remindedAt":"2027-01-24T15:00:00Z","remindEveryDays":7,"assignedStaffId":"s1""#;
        const LOAN: &str = r#""id":"l2","clientId":"c","title":"Bank loan package","note":"","dueDate":"2027-02-01","status":"open","complete":false,"done":0,"total":2,"createdAt":"2027-01-20T16:00:00Z","createdByKind":"staff","createdBy":"kim@example-cpa.test","remindEveryDays":0,"assignedStaffId":null"#;
        let template = |id: &str, name: &str| {
            format!(
                r#"{{"id":"{id}","name":"{name}","note":"Please send these by March 15.","items":{ITEMS}}}"#
            )
        };
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                if r.read_line(&mut first).is_err() {
                    continue;
                }
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = n.trim().parse().unwrap_or(0);
                    }
                }
                let mut sink = vec![0u8; len];
                let _ = r.read_exact(&mut sink);
                let path = first
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let body = match path.trim_start_matches("/api/firm/v1") {
                    "/request-lists" => format!(r#"{{"lists":[{{{LIST}}},{{{LOAN}}}]}}"#),
                    "/request-lists/l1" => format!(r#"{{"list":{{{LIST},"items":{ITEMS}}}}}"#),
                    "/request-staff" => r#"{"staff":[{"id":"s1","name":"Kim Lee","email":"kim@example-cpa.test"},{"id":"s2","name":"Pat Ortiz","email":"pat@example-cpa.test"}]}"#.to_string(),
                    "/request-templates" => format!(
                        r#"{{"templates":[{},{}]}}"#,
                        template("t1", "1040 year-end documents"),
                        template("t2", "Bank loan package")
                    ),
                    "/status" => r#"{"version":"0.5.0","firmName":"Example CPA","portalUrl":"http://127.0.0.1","features":["requests"]}"#.to_string(),
                    _ => r#"{"ok":true}"#.to_string(),
                };
                let mut w = s;
                let _ = write!(
                    w,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://127.0.0.1:{port}")
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
            .with_max_steps(crate::ui::test_util::MAX_STEPS)
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

        // 5. Find documents in Browse, typed in two goes, then found.
        h.get_by_role_and_label(Role::Button, "Browse").click();
        h.run_steps(8);
        h.get_by_label("Find documents…").click();
        h.run_steps(4);
        h.get_by_label("Words").click();
        h.step();
        for (i, part) in ["1099", "-INT 2025"].iter().enumerate() {
            h.get_by_label("Words").type_text(part);
            h.run_steps(4);
            shot(&mut h, &format!("05-search-{i}"));
        }
        h.get_by_label("Find").click();
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
