# filer-intake — VENDORED

## Source

| | |
|---|---|
| Repo | `KisaesDevLab/Vibe-Filer-Desktop` (private) |
| Branch | `master` |
| Commit | `1c6d2e8` on `master` (a test fix only, 2026-11-14); before: `e1bbe7f` (V40: History undo check on a worker, inbox watcher asks less, 2026-11-14); before: `e532dae` (V39: Conflicts draws only the rows in view, 2026-11-14); `1e60c03` (V37, V38: Stop using the CSV, partner filter, 2026-11-12); `9f40157` (V36); `229354a` (V35); `374edfd` (V34); `d48341f` (page count under the file name); `d0068ae` (V32, V33); `dd78a06` (V31); `4a15680` (column lines); `bab3a61` (V30); `e80a8b1` (Unsupported strip width); `ac56db4` (V29); `c5e2f70` (QA of V27-V28); `e165e56` (V27-V28); `6417249` (V23-V26); `f2504fe` (V22); `29db764` (V21); `6433c04` on `master` ("feat(host): Setup lock and \"New client…\" for a program that embeds Filer", 0.2.6 + the Intake preview pane, ticked-row actions and host hooks, 2026-11-04) |
| Vendored | 2026-11-02, P00 step 2 at `ea4e726` (0.2.4); re-vendored 2026-11-04, PHASES PA-26 |
| Upstream tests | 238 at this commit (237 at `dd78a06`); all pass after the split (filer-core 65 · filer-pdf 32 · filer-intake 141) |

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

5. **Tests only** (owner 2026-11-16, PA-87: "make any necessary changes to the Filer necessary to
   ensure stability in the Files app"): the eight `settle` helpers in `filer-intake/src/app.rs`
   wait by the clock (180 s at most) instead of a fixed 300 to 600 frames. The work they wait
   for runs on other threads, and a few seconds was not always enough on a machine busy with
   something else (`twenty_file_batch_resolves_to_ready`, `combine_commit_and_undo_through_the_ui`
   failed now and then with nothing wrong). Also in the tests of `app.rs`: a header tab is
   opened with `press_tab` (pressed again until it is the one shown: a press on a frame where
   the header is drawn again did nothing) and four fixed five-frame waits are `wait_for_label`.
   To go upstream with the next re-vendor, or be applied again after it.

6. **Logic, for safety** (owner 2026-11-16, PA-89, after a review of the file-moving code; to go
   upstream). Each is marked `Vibe Files (PA-89)` where it is:
   - `undo.rs`: a split's piece is deleted on undo only while its original is still there
     (where the batch kept it; or in its inbox with the size it had, `PartialSplit.size`); a
     combined PDF is kept when one of its originals that could not be parked
     (`FiledItem.unparked`) is gone; a client folder that still holds a file of the batch
     (item, copy or zip entry) keeps its client file; a batch with only a kept overwritten
     file can be undone, and its folder is marked (`UNRESTORED_MARK`) so later commits do
     not clear it.
   - `commit.rs` `run_unpack`: a zip with an entry that failed is put aside in the inbox's
     `_combined` folder (not unpacked twice, not cleared by the next commit; Undo puts it back).
   - `commit.rs` `transfer`: says so when the copy could not be taken back either.
   - `commit.rs` roll-back: an overwritten file that could not be put back stays in the batch's
     backups.
   - `plan.rs` / `filer-core/src/parse.rs` (the owner's four choices): a name match is not
     taken when the name carries an ID no client has, or two clients match; an ID found as a
     word is taken only as a `_` part of its own (first or second; a two-part ID also last), when read by the
     UltraTax layout, or with the client's name in the file name (`id_is_a_part_of_its_own`);
     only a token shaped like the list's IDs, and no year, counts as "an ID no client has"
     (`looks_like_a_client_id`); two different years in what rules read are not guessed
     between (`years_in`; never against an UltraTax tax year); a folder is bound on commit
     only by name (`name_score` at the auto threshold) and when it is the only one.

   - `filer-core/src/datafile.rs` `write_atomic` (marked `Vibe Files (PA-94)`, owner's report
     2026-11-17: "last_batch.json could not be written: Access is denied"): a temp file of
     its own name per save, and a refused rename tried again for under half a second.

Logic edits are those of item 6 only. Check with:
`diff -r <upstream>/src crates/filer-*/src` (only the four attribute lines and the `settle`
helpers of item 5 differ).

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
  Then `e165e56` → `c5e2f70` (QA of V27-V28): spreadsheets read cell by cell within the caps,
  `.xls` size-checked before opening, `.ods` not shown, picture size limits (image-to-PDF too),
  two reads at a time, Word/RTF/HTML text fixes, slide order, stricter ID search.
  `fileview.rs`, `imagepdf.rs` (filer-pdf), `parse.rs` (filer-core), `ui/fileview.rs`
  (filer-intake) copied; still only the four attribute lines differ; upstream's 232 tests pass.
  Then `c5e2f70` → `ac56db4` (V29): `ClientList::match_by_id_loose` (an ID with or without `_`,
  `-`, spaces, any case finds the one client that reads that way; the row shows the list's own
  ID). `clients.rs`, `parse.rs` (filer-core), `plan.rs` (filer-intake) copied; still only the
  four attribute lines differ; upstream's 234 tests pass.
  Then `ac56db4` → `e80a8b1`: the Unsupported strip's header is cut to the table's width and its
  list scrolls (21 long names widened the table past the window, hiding the scroll bar and the
  side pane). `app.rs`, `ui/preview.rs` (filer-intake) copied; upstream's 235 tests pass.
  Then `e80a8b1` → `bab3a61` (V30): the queue's Date column (last-modified, read on a worker) and
  headings that sort. `app.rs`, `ui/preview.rs` (filer-intake) copied; upstream's 236 tests pass.
  Then `bab3a61` → `4a15680`: lines after the tick box, Date, Year and Destination columns (egui draws
  them only after resizable columns). `ui/preview.rs` (filer-intake) copied; upstream's 236 tests pass.
  Then `4a15680` → `dd78a06` (V31, V32): with no client's ID in a name, an ID-shaped part near the
  end (`HAWO6056`, `3280_TIAM`) instead of the year; the toolbar's Tick matched. `parse.rs`
  (filer-core), `app.rs`, `ui/preview.rs` (filer-intake) copied; upstream's 237 tests pass.
  Then `dd78a06` → `d0068ae` (V32, V33): Tick matched became Tick ready (rows whose status is Ready);
  a personal inbox never counts its sub-folders (the watcher keeps one setting per inbox).
  `intake.rs`, `app.rs`, `ui/setup.rs`, `ui/preview.rs` (filer-intake) copied; upstream's 238 tests pass.
  Then `d0068ae` → `d48341f`: the page count on its own line under the file name, at the right.
  `app.rs`, `ui/preview.rs` (filer-intake) copied; upstream's 238 tests pass.
- **2026-11-11, PA-44: `d48341f` → `374edfd` (V34).** Setup's path check allows an inbox inside the
  client root in a top-level `_`, `.` or `~` folder (the root scan leaves those out), so Vibe's
  `_Portal inbox` no longer blocks saving Setup. `config.rs` (filer-core), `ui/setup.rs`,
  `docs/UI_SPEC.md` (filer-intake) copied; still only the four attribute lines differ; upstream's
  238 tests pass.
- **2026-11-12, PA-45: `374edfd` → `229354a` (V35).** Map CSV columns' Optional **Partner** column:
  its values become `{Partner}` whatever the header (`ColumnMap.partner`); Setup shows
  `Partner → <column>`. `config.rs`, `clients.rs`, `relationships.rs` (filer-core), `split.rs`
  (filer-pdf), `app.rs`, `plan.rs`, `sentinel.rs`, `undo.rs`, `ui/csv_wizard.rs`, `ui/setup.rs`,
  `docs/UI_SPEC.md` (filer-intake) copied (most only gain `partner: None` in test literals); still
  only the four attribute lines differ; upstream's 240 tests pass.
- **2026-11-12, PA-49: `229354a` → `9f40157` (V36).** `FilerApp::reload_clients`: the client list and
  relationships read again in place, the review kept. `app.rs` (filer-intake) copied; still only
  the four attribute lines differ; upstream's 240 tests pass.
- **2026-11-12, PA-50/PA-52: `9f40157` → `1e60c03` (V37, V38).** Stop using the CSV; a CSV gone keeps
  the list with one note; `reload_clients` reads the CSV setting from config.json; the review's
  Partner filter and its host hooks. `clients.rs` (filer-core), `app.rs`, `ui/preview.rs`,
  `ui/setup.rs`, `docs/UI_SPEC.md` (filer-intake) copied; still only the four attribute lines
  differ; upstream's 243 tests pass.

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

- **2026-11-14: `1e60c03` → `e532dae` (V39).** The owner's Conflicts tab froze the window: with thousands of folders that have no ID in their name, every client's name was searched once per folder before the first frame (51 s for 7,600 folders and 8,100 clients in upstream's timing test; 0.09 s after). Only the rows in view are drawn now, and "Rename to template name" reads the root's listing once. Files: `app.rs` (the ignored timing test `conflicts_at_scale`), `ui/conflicts.rs`. Upstream commit is local, not pushed.

- **2026-11-14: `e532dae` → `e1bbe7f` (V40).** Two findings of the Intake review (QA I1, I9). History asked the share about every file of the last batch on every repaint: the undo check now runs on a worker, at most every 5 s. The inbox watcher asked the share about every waiting file every 30 s in every session: it lists the inboxes once and asks only about files the listing does not show. Files: `intake.rs`, `ui/history.rs`. Upstream commit is local, not pushed.

- **2026-11-14: `e1bbe7f` → `1c6d2e8`.** A test only: `combine_commit_and_undo_through_the_ui` failed now and then in full runs because its first tick click was lost while the rows were still settling; the helper now clicks until the tick has changed. No fault in Combine. File: `app.rs`.
