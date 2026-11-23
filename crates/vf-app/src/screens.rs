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

    /// PA-103 walkthrough against a real Vibe Portal (not a fake): the to-do window drives every
    /// firm step, and `node client.mjs` the client's steps in a browser. Set `VF_WALK_URL`,
    /// `VF_WALK_KEY`, `VF_WALK_LOG` (the portal's log, EMAIL_PROVIDER=console), `VF_WALK_CLIENT`
    /// (client.mjs), `VF_WALK_PDF` (the file the client sends) and `VF_WALK_DIR` (output).
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
