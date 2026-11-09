# filer-intake — VENDORED

## Source

| | |
|---|---|
| Repo | `KisaesDevLab/Vibe-Filer-Desktop` (private) |
| Branch | `master` |
| Commit | `e165e56` on `master` ("fix(parse): client ID in the last, then first, 14 characters; two-part IDs (V28)", 2026-11-09); before: `6417249` (V23-V26); `f2504fe` (V22); `29db764` (V21); `6433c04` on `master` ("feat(host): Setup lock and \"New client…\" for a program that embeds Filer", 0.2.6 + the Intake preview pane, ticked-row actions and host hooks, 2026-11-04) |
| Vendored | 2026-11-02, P00 step 2 at `ea4e726` (0.2.4); re-vendored 2026-11-04, PHASES PA-26 |
| Upstream tests | 227 at this commit (215 at `6417249`); all pass after the split (filer-core 64 · filer-pdf 27 · filer-intake 136) |

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
4. Package versions kept at upstream's (`0.2.6`) so `CARGO_PKG_VERSION` text is unchanged.

No logic edits. Check with:
`diff -r <upstream>/src crates/filer-*/src` (only the four attribute lines differ).

## File → crate

| Upstream | Crate |
|---|---|
| `src/parse.rs` `config.rs` `datafile.rs` `clients.rs` `template.rs` `worker.rs` `rules.rs` `rulebuild.rs` `relationships.rs`, `assets/default_template.json` | `filer-core` |
| `src/pdf.rs` `split.rs` `imagepdf.rs` `fileview.rs`, `assets/pdfium.dll` + licenses | `filer-pdf` |
| `src/app.rs` `intake.rs` `sentinel.rs` `zipimport.rs` `plan.rs` `commit.rs` `undo.rs` `oplog.rs` `recovery.rs` `logging.rs` `pid.rs` `ui/*`, `assets/fonts/*`, `tests/fixtures/*`, `UI_SPEC.md` + `docs/mockups/` (reference) | `filer-intake` |
| `src/main.rs` | not vendored |

Deviation from the plan's list: `sentinel.rs` is in `filer-intake`, not `filer-core`, because it
calls `commit` (folder renames are recorded in `last_batch.json`), and `commit` → `plan` pulls in
the rest of intake. Moving it would need a logic edit (Q-16: merge rather than edit).

## Re-vendor log

- **2026-11-04, PA-26: `ea4e726` (0.2.4) → `f0a3e28` (0.2.6 + Intake preview pane and ticked-row
  Set client… / Apply rule…, built upstream at the owner's request).** Files copied over with LF
  endings; the four attributes re-applied; `datafile.rs` (new, 0.2.6) added to filer-core and
  re-exported by filer-intake; filer-pdf now also re-exports `rules` (its tests use it). Vibe
  side: `rules::evaluate` is test-only upstream now (Vibe calls `first_match` + `evaluate_rule`);
  `first_match` takes the "read as UltraTax" flag; Filer's own UltraTax name reading (0.2.5) is
  switched off in the settings Vibe hands Filer (Vibe's print capture is the one UltraTax set-up);
  print capture's managed rules now go ahead of all Filer rules (0.2.5 moved firm rules such as
  "Engagement Letter" before its UltraTax rule and then removed that rule).

- **2026-11-04, PA-26 (2): `f0a3e28` → `6433c04`: host hooks (V20).** `set_setup_lock` (Vibe
  locks Intake › Setup with the admin password) and `offer_new_client` / `take_new_client_request`
  / `client_created` (Vibe's New client dialog for a row whose client is not found). Copied as
  before; nothing else changed.

- **2026-11-05, PA-30: `6433c04` → `29db764`: host hooks (V21).** `offer_trash` /
  `take_trash_request` / `files_taken` (Intake's Move to trash, into Vibe's cabinet trash);
  `unnamed_files` / `suggest` (Vibe's form reading for files no rule names, Q-61); the row rule
  menu and Apply rule… with a Type to search box. `app.rs`, `ui/preview.rs`, `ui/widgets.rs`
  copied; the diff against upstream is still only the four attribute lines; upstream's 207 tests
  pass.

- **2026-11-05, PA-32: `29db764` → `f2504fe`: personal inboxes and V21 QA (V22).**
  `set_personal_inboxes` (a person's own inboxes, watched besides the shared ones, never saved
  into the shared settings; Preview's Shared inbox | My inbox tabs, each its own batch); files of
  an inbox no longer watched leave the queue; rows keep their ids by file; Enter with nothing
  typed keeps a menu's choice; Move to trash only for files from an inbox; host notes leave with
  their files. `app.rs`, `intake.rs`, `ui/preview.rs`, `ui/widgets.rs` copied; still only the
  four attribute lines differ; upstream's 208 tests pass.

- **2026-11-09, PA-37: `f2504fe` → `6417249`: Preview grid, file types, Import tabs, open
  (V23-V26).** The Preview table is an `egui_extras` table (columns stay aligned); Import has
  Shared inbox | My inbox; a double-clicked file name opens through `offer_open` /
  `take_open_request` (Vibe routes it to Browse's shell, not the Viewer, which edits catalogued
  documents only); Setup › Inboxes › Also file and File images as PDFs (new upstream module
  `imagepdf.rs`, put in filer-pdf and re-exported by filer-intake). `app.rs`, `intake.rs`,
  `ui/import.rs`, `ui/preview.rs`, `ui/setup.rs`, `ui/widgets.rs` (filer-intake), `config.rs`
  (filer-core), `pdf.rs` and `imagepdf.rs` (filer-pdf) copied; the two `pdf.rs` attributes
  re-applied; workspace `image` gains png/tiff/bmp/gif and `tiff` 0.11.3 is added (filer-pdf;
  `image` also a filer-intake dev-dependency for its tests), `commit.rs` too after the QA fixes;
  workspace `windows-sys` gains Win32_UI_Shell and Win32_UI_WindowsAndMessaging (ShellExecute).
  Still only the four attribute lines differ; upstream's 215 tests pass.

- **2026-11-09, PA-38: `6417249` → `e165e56`: Office preview and client IDs (V27, V28).** The
  Preview side pane shows the inside of spreadsheets, Word / PowerPoint, RTF / text, mail
  (`.eml`, `.msg`) and pictures (new upstream modules `fileview.rs`, put in filer-pdf and
  re-exported by filer-intake, and `ui/fileview.rs`, which Vibe's Browse preview also uses); a
  name whose client ID is not where Name_ID_rest or the UltraTax layout expects it is searched
  for a client's ID in its last 14 characters, then its first 14, two-part IDs first (`3280_TIAM`,
  `3280TIAM`). `app.rs`, `plan.rs`, `ui/fileview.rs`, `ui/mod.rs`, `ui/peek.rs`,
  `ui/preview.rs` (filer-intake), `parse.rs` (filer-core), `fileview.rs`, `imagepdf.rs`
  (filer-pdf) copied. Workspace dependencies added: `calamine` 0.38 (dates), `cfb` 0.16,
  `mailparse` 0.18, `quick-xml` 0.42 (filer-pdf; MIT / 0BSD), `rust_xlsxwriter` 0.102 (dev, MIT
  OR Apache-2.0). Still only the four attribute lines differ (three UI files differ in line
  endings only); upstream's 227 tests pass.

## Re-vendoring

Only as a PA-xx addendum step: copy the new upstream files over, re-apply the list above, run the
full gate, update this file. Never edit vendored files to "catch up" (QUESTIONS Q-18).

## Upstream safety rules (verbatim, Filer Desktop CLAUDE.md §7)

Every document move in Vibe Files goes through this crate and keeps these rules:

- Never call `fs::remove_file` on anything except a source file whose copy has been hash-verified, a copy being rolled back in the same batch, or during undo on a file the app itself created whose current hash matches the logged hash.
- Never `remove_dir_all`. Only `remove_dir` on empty dirs the app created in the batch being undone.
- Every path written is checked to be inside `root` or an inbox (canonicalize, prefix check).
  Owner exception (2026-11-01, QUESTIONS X53): a file the user explicitly added by drag-drop or
  Browse may be removed from its original location after it has been moved or hash-verified
  into the client folder, like an inbox file.
- Reject templates that produce `..`, reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9), trailing dots/spaces, or paths > 240 chars.
