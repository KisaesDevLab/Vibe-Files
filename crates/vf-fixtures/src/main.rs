//! `cargo run -p vf-fixtures [-- <out> [seed]]` → `fixtures/cabinet/` by default.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cabinet")
    });
    let seed = args.next().map(|s| s.parse()).transpose()?.unwrap_or(42);
    let s = vf_fixtures::generate(&out, seed)?;
    println!(
        "{} clients, {} documents → {}",
        s.clients,
        s.docs.len(),
        out.display()
    );
    Ok(())
}
