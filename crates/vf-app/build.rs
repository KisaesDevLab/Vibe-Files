//! Embeds the short commit hash as `VF_COMMIT` (PA-100 R2 step 4): a build identifier shown in the
//! footer beside the version, never a link. The release script sets `VF_COMMIT`; otherwise it is
//! read from git, and `unknown` when there is no git (a source copy without history).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    // The exe's icon (PA-102). Only for a Windows target; tests and clippy elsewhere skip it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=vibe-files.rc");
        println!("cargo:rerun-if-changed=assets/vibe-files.ico");
        embed_resource::compile("vibe-files.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the icon resource (vibe-files.rc) did not compile");
    }
    println!("cargo:rerun-if-env-changed=VF_COMMIT");
    // Rebuild when HEAD moves (a new commit or a branch switch), in a worktree too. Only paths
    // that exist are named (a missing one would rerun every build). A branch kept only in
    // packed-refs (after `git gc`, or a fresh clone) has no loose ref file yet: its folder is
    // watched instead, so the next commit, which writes the loose ref there, is seen.
    let mut watch = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(r);
    }
    for p in watch {
        let Some(path) = git(&["rev-parse", "--git-path", &p]) else {
            continue;
        };
        let mut path = std::path::PathBuf::from(path);
        // Never above `refs` (the git folder itself changes on every `git add`).
        while !path.exists()
            && p.starts_with("refs/")
            && path.file_name().is_some_and(|n| n != "refs")
        {
            path.pop();
        }
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let commit = std::env::var("VF_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&["rev-parse", "--short=9", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=VF_COMMIT={commit}");
}
