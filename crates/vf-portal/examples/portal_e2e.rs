//! Drives a running Vibe Portal through the same calls Vibe Files makes, one step per run, for
//! the PA-37 end-to-end check (`scripts/portal-e2e.sh`). Not shipped.
//!
//! VF_PORTAL_URL and VF_PORTAL_KEY name the portal. Steps:
//!   status
//!   client <client-id> <code> <name>
//!   invite <client-id> <email>
//!   users <client-id>                                   -> one identity id per line
//!   share <client-id> <doc-id> <path> <shown-id> [<hidden-id>]
//!   vis <file-id> <identity-id> <true|false>
//!   viewed <doc-id> <identity-id>                       -> "opened" or "not opened"
//!   fetch <cabinet-root>                                -> one saved path per line
//!   list-send <client-id> <code> <name> <rev> <head> [<path>...]
//!                                                       -> "sent N cleared M kept K" (PA-80): this PC
//!                                                          has only that client, with those
//!                                                          documents in B2
//!   list-clear-all                                      -> how many clients had a list

use std::collections::BTreeMap;
use std::path::Path;

use vf_portal::{Portal, PortalSettings};

fn main() {
    let url = std::env::var("VF_PORTAL_URL").expect("VF_PORTAL_URL");
    let key = std::env::var("VF_PORTAL_KEY").expect("VF_PORTAL_KEY");
    let s = PortalSettings::new("e2e", &url, &key, 0).expect("settings");
    let pc = std::env::var("VF_PC").unwrap_or_else(|_| "E2E-PC".into());
    let p = Portal::new(&s, &pc, "tester");
    let a: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or_default();
    let fail = |e: vf_portal::PortalError| -> ! {
        eprintln!("{}", e.message());
        std::process::exit(1)
    };
    match arg(0) {
        "status" => {
            let st = p.status().unwrap_or_else(|e| fail(e));
            println!("{} {}", st.firm_name, st.version);
        }
        "client" => p
            .put_client(arg(1), arg(2), arg(3), None)
            .unwrap_or_else(|e| fail(e)),
        "invite" => {
            p.invite(arg(1), Some(arg(2)), None, "", "")
                .unwrap_or_else(|e| fail(e));
        }
        "users" => {
            for u in p.users(arg(1)).unwrap_or_else(|e| fail(e)).users {
                println!("{} {}", u.identity_id, u.email.unwrap_or_default());
            }
        }
        "share" => {
            let path = Path::new(arg(3));
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let added = p
                .add(arg(1), arg(2), &name, Some(2025), path)
                .unwrap_or_else(|e| fail(e));
            let mut vis = BTreeMap::new();
            vis.insert(arg(4).to_string(), true);
            if !arg(5).is_empty() {
                vis.insert(arg(5).to_string(), false);
            }
            p.set_visibility(added.file_id(), &vis)
                .unwrap_or_else(|e| fail(e));
            println!("{}", added.file_id());
        }
        "vis" => {
            let mut vis = BTreeMap::new();
            vis.insert(arg(2).to_string(), arg(3) == "true");
            p.set_visibility(arg(1), &vis).unwrap_or_else(|e| fail(e));
        }
        "viewed" => {
            let f = p
                .file_for_doc(arg(1))
                .unwrap_or_else(|e| fail(e))
                .expect("on the portal");
            let u = f.users.iter().find(|u| u.identity_id == arg(2));
            println!(
                "{}",
                if u.is_some_and(|u| u.viewed_at.is_some()) {
                    "opened"
                } else {
                    "not opened"
                }
            );
        }
        "fetch" => {
            let r = vf_portal::inbox::fetch(Path::new(arg(1)), &p, "2026-11-09T15:00:00Z");
            for e in &r.problems {
                eprintln!("problem: {e}");
            }
            for x in &r.received {
                println!("{}", x.saved_as);
            }
            if !r.problems.is_empty() {
                std::process::exit(1);
            }
        }
        "list-send" => {
            let files = a
                .iter()
                .skip(6)
                .enumerate()
                .map(|(i, path)| vf_portal::listing::ListFile {
                    doc_id: format!("{}-doc-{i}", arg(1)),
                    path: path.clone(),
                    size: 100 + i as u64,
                    tax_year: (i % 2 == 0).then_some(2025),
                })
                .collect();
            let local = vf_portal::listing::Local {
                head_rev: arg(5).parse().expect("head"),
                live: [arg(1).to_string()].into(),
                me: p.actor_name(),
                clients: vec![vf_portal::listing::ClientList {
                    id: arg(1).into(),
                    code: arg(2).into(),
                    name: arg(3).into(),
                    rev: arg(4).parse().expect("rev"),
                    files,
                }],
            };
            let r = vf_portal::listing::send(&p, &local);
            if let Some(e) = &r.problem {
                eprintln!("problem: {e}");
                std::process::exit(1);
            }
            println!("sent {} cleared {} kept {}", r.sent, r.cleared, r.kept);
        }
        "list-clear-all" => {
            println!("{}", p.clear_cabinet_lists().unwrap_or_else(|e| fail(e)));
        }
        other => {
            eprintln!("unknown step: {other}");
            std::process::exit(2);
        }
    }
}
