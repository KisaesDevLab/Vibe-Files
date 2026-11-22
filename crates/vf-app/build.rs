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
    println!("cargo:rerun-if-env-changed=VF_COMMIT");
    // Rebuild when HEAD moves (a new commit or a branch switch), in a worktree too. Only files
    // that exist are named: a missing one (a ref kept only in packed-refs) would rerun every build.
    let mut watch = vec!["HEAD".to_string()];
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(r);
    }
    for p in watch {
        if let Some(path) = git(&["rev-parse", "--git-path", &p])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = std::env::var("VF_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&["rev-parse", "--short=9", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=VF_COMMIT={commit}");
}
