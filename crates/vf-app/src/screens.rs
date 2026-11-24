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
