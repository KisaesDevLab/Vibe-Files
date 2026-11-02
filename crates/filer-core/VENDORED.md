# filer-core — VENDORED

## Source

| | |
|---|---|
| Repo | `KisaesDevLab/Vibe-Filer-Desktop` (private) |
| Branch | `master` |
| Commit | `ea4e7267797ee869f05c3e5745700e13910a2e5b` ("chore: 0.2.4", 2026-11-02 10:19 -0600) |
| Vendored | 2026-11-02, P00 step 2 (`master` re-checked that day: still `ea4e726`) |
| Upstream tests | 194 at this commit; all pass after the split (filer-core 61 · filer-pdf 12 · filer-intake 121) |

Upstream is one binary crate with no `lib.rs`. The three `filer-*` crates are a mechanical split
of its `src/` (QUESTIONS Q-16). Upstream `main.rs` is not vendored; `vf-app` is the entry point.

## Changes from upstream (the complete list)

1. A new `lib.rs` per crate: `pub mod` for its modules, `pub use` re-exports of the other
   filer crates' modules so upstream `crate::clients`-style paths resolve unedited, and
   `#![allow(clippy::len_without_is_empty, clippy::result_unit_err, clippy::new_without_default)]`
   (lints that fire only because the code is now a library).
2. Four test-helper attributes `#[cfg(test)]` → `#[cfg(any(test, feature = "test-support"))]` so
   other crates' tests can use them: `filer-core/src/rules.rs` `seed_tax`, `tax_rules`;
   `filer-pdf/src/pdf.rs` `render_page`, `write_test_pdf`.
3. Assets moved beside the module that reads them, so `include_bytes!`/`include_str!`/
   `CARGO_MANIFEST_DIR` paths are unchanged.
4. Package versions kept at `0.2.4` so `CARGO_PKG_VERSION` text is unchanged.

No logic edits. Check with:
`diff -r <upstream>/src crates/filer-*/src` (only the four attribute lines differ).

## File → crate

| Upstream | Crate |
|---|---|
| `src/parse.rs` `config.rs` `clients.rs` `template.rs` `worker.rs` `rules.rs` `rulebuild.rs` `relationships.rs`, `assets/default_template.json` | `filer-core` |
| `src/pdf.rs` `split.rs`, `assets/pdfium.dll` + licenses | `filer-pdf` |
| `src/app.rs` `intake.rs` `sentinel.rs` `zipimport.rs` `plan.rs` `commit.rs` `undo.rs` `oplog.rs` `recovery.rs` `logging.rs` `pid.rs` `ui/*`, `assets/fonts/*`, `tests/fixtures/*`, `UI_SPEC.md` + `docs/mockups/` (reference) | `filer-intake` |
| `src/main.rs` | not vendored |

Deviation from the plan's list: `sentinel.rs` is in `filer-intake`, not `filer-core`, because it
calls `commit` (folder renames are recorded in `last_batch.json`), and `commit` → `plan` pulls in
the rest of intake. Moving it would need a logic edit (Q-16: merge rather than edit).

## Re-vendoring

Only as a PA-xx addendum step: copy the new upstream files over, re-apply the list above, run the
full gate, update this file. Never edit vendored files to "catch up" (QUESTIONS Q-18).
