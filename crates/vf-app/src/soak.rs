//! `vibe-files soak` (PHASES P13 step 3): memory and handle leaks over a long run. On a synthetic
//! cabinet it repeats what a working day does, in one process: open the cabinet, read the tree
//! and grid, search, edit metadata, replace content (a new version), rename, push and pull
//! against an in-memory bucket, verify the audit chain. Every minute it writes the working set,
//! private bytes and handle count to `soak.csv`; at the end it compares the first and last hour
//! (or tenth). Nothing here touches a real cabinet.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

/// (working set MB, private MB, handles) of this process.
pub fn usage() -> (f64, f64, u32) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut m = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    let mut handles = 0u32;
    // SAFETY: the pseudo-handle of this process; `m` is sized as `cb` says.
    unsafe {
        let p = GetCurrentProcess();
        let _ = GetProcessMemoryInfo(p, &mut m as *mut _ as *mut _, m.cb);
        let _ = GetProcessHandleCount(p, &mut handles);
    }
    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    (mb(m.WorkingSetSize), mb(m.PrivateUsage), handles)
}

fn put(root: &Path, rel: &str, body: &str) -> Result<()> {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("has a folder"))?;
    std::fs::write(p, body)?;
    Ok(())
}

/// Runs for `hours`, in `dir` (a new folder). Returns the exit code: 0 = no growth beyond the
/// allowance, 6 = memory or handles kept growing.
pub fn run(dir: &Path, hours: f64, out: &mut dyn Write) -> Result<i32> {
    std::fs::create_dir_all(dir)?;
    let root = dir.join("Clients");
    let data = dir.join("data");
    let req = crate::session::CreateRequest {
        root: &root,
        name: "Soak",
    };
    {
        let mut o = crate::session::create(&data, "soak", &req)?;
        for c in 0..20 {
            let p = vf_fs::newclient::plan(
                &o.conn,
                &root,
                &vf_fs::newclient::NewClient {
                    ext_id: format!("{:04}", 1000 + c),
                    name: format!("Soak Client {c:02}"),
                    entity_type: None,
                },
            )
            .map_err(anyhow::Error::msg)?;
            vf_fs::newclient::create(&mut o.conn, &root, &p, "soak").map_err(anyhow::Error::msg)?;
            for d in 0..25 {
                put(
                    &root,
                    &format!(
                        "Soak Client {c:02} ({:04})/Income Tax/2025/doc {d}.pdf",
                        1000 + c
                    ),
                    &format!("invented interest dividend wages {c} {d}"),
                )?;
            }
        }
        vf_fs::rebuild::rebuild(&mut o.conn, &root, "soak")?;
    }
    let cab = vf_fs::cabinet::read(&root).map_err(anyhow::Error::msg)?;
    // A bucket on disk: an in-memory one would grow the process with every push.
    let remote = vf_sync::Remote::new(
        vf_sync::Remote::folder(&dir.join("bucket"))?,
        &cab.cabinet_id,
    );
    let keys = vf_sync::setup::init(
        &root,
        &remote,
        "soak passphrase, not a real one",
        vf_sync::setup::DEFAULT_KDF,
    )?;
    let s = vf_sync::Sync {
        remote,
        keys,
        cabinet_id: cab.cabinet_id.clone(),
        device_id: "soak".into(),
        device_name: "SOAK".into(),
    };
    let csv = dir.join("soak.csv");
    let mut log = std::fs::File::create(&csv)?;
    writeln!(log, "minute,iterations,working_set_mb,private_mb,handles")?;
    let start = Instant::now();
    let end = Duration::from_secs_f64(hours * 3600.0);
    let mut next = Duration::ZERO;
    let mut samples: Vec<(f64, f64, u32)> = vec![];
    let mut i: u64 = 0;
    while start.elapsed() < end {
        i += 1;
        let mut o = crate::session::open(&data, "soak", &root)?;
        let snap = vf_core::browse::load(&o.conn)?;
        let _ix = vf_core::browse::Index::build(&snap);
        let q = vf_index::search::Query {
            text: "interest".into(),
            ..Default::default()
        };
        let _ = vf_index::search::search(&o.conn, &q, 100)?;
        let id = &snap.docs[(i as usize * 7919) % snap.docs.len()].id;
        let _ = vf_fs::docmeta::edit(
            &mut o.conn,
            &root,
            std::slice::from_ref(id),
            &vf_fs::docmeta::MetaEdit {
                tax_year: Some(Some(2020 + (i % 6) as i32)),
                ..Default::default()
            },
            "soak",
        );
        let tmp = vf_fs::content::tmp_file(&root, "pdf").map_err(anyhow::Error::msg)?;
        std::fs::write(&tmp, format!("invented content {i}"))?;
        let _ = vf_fs::content::replace(
            &mut o.conn,
            &root,
            "soak",
            id,
            &tmp,
            vf_fs::content::Change {
                label: "soak",
                created_by: "soak",
                reason: "soak test",
                detail: serde_json::json!({}),
                extra: vec![],
            },
        );
        if i.is_multiple_of(10) {
            vf_sync::push::push(&mut o.conn, &root, &s, None)?;
            vf_sync::pull::pull(&mut o.conn, &root, &s)?;
        }
        let _ = vf_core::audit::verify(&o.conn)?;
        drop(o);
        if start.elapsed() >= next {
            let u = usage();
            let minute = start.elapsed().as_secs() / 60;
            writeln!(log, "{minute},{i},{:.1},{:.1},{}", u.0, u.1, u.2)?;
            log.flush()?;
            samples.push(u);
            next += Duration::from_secs(60);
        }
    }
    let u = usage();
    samples.push(u);
    // Compare the first and the last tenth of the run (after a warm-up sample).
    let n = samples.len();
    let k = (n / 10).max(1);
    let avg = |s: &[(f64, f64, u32)]| {
        let l = s.len() as f64;
        (
            s.iter().map(|x| x.1).sum::<f64>() / l,
            s.iter().map(|x| x.2 as f64).sum::<f64>() / l,
        )
    };
    let first = avg(&samples[1.min(n - 1)..(1 + k).min(n)]);
    let last = avg(&samples[n - k..]);
    writeln!(
        out,
        "{i} iterations in {:.1} h · private memory {:.1} → {:.1} MB · handles {:.0} → {:.0} · log {}",
        start.elapsed().as_secs_f64() / 3600.0,
        first.0,
        last.0,
        first.1,
        last.1,
        csv.display()
    )?;
    // Allowance: caches settle; steady growth beyond this is a leak.
    let grew = last.0 > first.0 * 1.25 + 20.0 || last.1 > first.1 * 1.25 + 50.0;
    if grew {
        writeln!(out, "memory or handles kept growing: see the log")?;
    }
    Ok(if grew { 6 } else { 0 })
}
