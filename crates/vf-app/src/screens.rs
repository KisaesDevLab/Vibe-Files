//! UI review helper (P13 QA): renders every tab and Admin page of the app on the generated
//! fixture cabinet to PNG files, for a person to look at. Not a regular test:
//! `cargo test -p vf-app screens -- --ignored`, with `VF_SCREENS_DIR` set to the output folder.

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
    }
}
