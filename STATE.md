# Vibe Files — STATE.md

Updated by Claude Code at the end of every session. Human edits welcome.

## Current phase

**P01 — Cabinet browse, client master, doc types, tax-year metadata** — all 8 steps built (2026-11-02); every Done-when box is covered by a test. **Waiting on the owner's hands-on UI check on a Windows PC** before P01 is called done (owner 2026-11-02: UI is not tested under Wine; that happens on Kurt's PC through Claude Code).

P00 is **done**: the last box (all 194 vendored Filer tests on real Windows) passed on GitHub `windows-latest`, CI run 41506666554.

## Next step

1. On Kurt's Windows PC (Claude Code): `cargo test --workspace -- --test-threads=2`, then run `vibe-files.exe` and walk P01 by hand:
   - Create a cabinet (empty folder, firm name, 12+ char passphrase); quit; relaunch → Unlock.
   - Open existing cabinet on the fixture (`cargo run -p vf-fixtures -- <dir>`), check the tree, counts, sort, Ctrl/Shift-click, arrows, Space, Ctrl+A.
   - + New client; Import clients… from a CSV (bind an existing folder, create the rest).
   - Client panel: entity type, add/remove a relationship with a share %.
   - Admin → Doc types: add, edit, archive, restore.
   - Select several documents → inspector fields → Apply to N → Undo last / Ctrl+Z.
   - Rename… with the default template; check the preview's keep-both rows; undo.
   - Views counts, Pin / Unpin, Add to favorites, double-click open, Open in Explorer, Copy path.
   Fix what that finds, then mark P01 done.
2. Start P02 step 1 (Intake tab mounting the vendored Filer Desktop engine).

## Built this session (2026-11-02, P01)

| Step | Commit | What |
|---|---|---|
| 1 | feat(vf-app) | Cabinet wizard: create / attach (rebuild) / unlock. `vf-security::keys`: random catalog key wrapped by the recovery passphrase (Argon2id + XChaCha20-Poly1305) in `<data>/keys.json`. `<data>/config.toml` (device id moved there). `cabinet.json` gains `firm_name`. CLI takes `VF_PASSPHRASE`. |
| 2 | feat(vf-app) | Browse tab: `vf-core::browse` queries (tree with live counts, Permanent / year buckets, folders derived from paths, grid per scope); grid with sort, multi-select, keyboard; inspector details + last 3 audit rows. |
| 3 | feat(vf-fs) | `FileOp::WriteNew`. `vf-fs::clients`: new client from `_Vibe/template.json` (Filer format) + SentinelV1 in one Transaction; CSV import via filer-core with folder discovery; entity type; relationships with share % mirrored to `_Vibe/relationships.json` (Filer format) and restored by rebuild. App commits mirror audit rows to `_Vibe/audit/*.jsonl`. |
| 4 | feat(vf-core) | `vf-core::doctype`: 41 seeded types with default folders; add / edit / archive with undo; Admin tab (doc types, audit verify). |
| 5 | feat(vf-core) | `vf-core::meta::bulk_edit`: one Transaction, one audit batch, one undo; inspector Mixed fields, Apply to N, Undo last (Ctrl+Z). |
| 6 | feat(vf-fs) | `vf-fs::rename`: Filer filename tokens, keep-both suffix, refuses missing values; Rename… dialog with preview. |
| 7–8 | feat(vf-app) | Migration 002 (user_pin, user_favorite, user_recent); Views (Recent, Pinned, Needs doc type, Needs tax year, Received this week) + favorites; Open (ShellExecuteW), Open in Explorer, Copy path, Pin. |

Gate used this session: `cargo clippy --workspace --all-targets -D warnings` for the Windows target, and the non-UI tests (`vf-core`, `vf-fs`, `vf-security`, `vf-fixtures`, `vf-app` session/config/cli/format) under Wine. The `vf-app` egui tests were last run under Wine at step 2 and were not run after (owner request); GitHub CI runs them on real Windows.

## Built this session (2026-11-02, P00)

| Step | Commit | What |
|---|---|---|
| 1 | feat(vf-app) | Workspace (edition 2024, deps pinned to Filer's lockfile); window with header, six tabs, footer; kittest. |
| 2 | feat(filer) | Filer Desktop `ea4e726` vendored as `filer-core`/`filer-pdf`/`filer-intake`, no logic edits; 194 upstream tests pass. `VENDORED.md` per crate. |
| 3 | docs | `docs/SENTINEL_V1.md` (from Filer `sentinel.rs`, doc example tested against it), `docs/BUCKET_LAYOUT.md` v1 draft. |
| 4 | feat(vf-core) | SQLCipher catalog, migration 001, keyed backup before migrate, append-only audit triggers. |
| 5 | feat(vf-core) | `Transaction` + `FileOps`; `vf-fs::fileops` uses Filer `transfer`/`copy_verified`; commit→undo→commit byte-identical (rename and copy-verify). |
| 6–7 | feat(vf-fs) | Audit hash chain + `vibe-files audit verify`; idempotent `vibe-files rebuild <cabinet>` (versions via sidecars, moves re-linked by hash, tombstones). |
| 8 | feat(vf-fixtures) | Deterministic 5-client / 60-PDF fixture cabinet with never-issued SSNs/EINs. |

Dev box: Linux container; gate is `scripts/check-wine.sh` (mingw cross-build, tests under Wine).

## Built this session (2026-11-02, PA-12)

- `.github/workflows/ci.yml`: Windows gate on GitHub Actions (fmt, clippy, test).
- `.github/workflows/release.yml` + `packaging/README.txt`: portable zip (`vibe-files.exe`, `pdfium.dll`, licenses, SHA-256 sums). Tag `v0.1.0` to publish a pre-release; Actions → release → Run workflow for a test build.

## Prerequisites before P00 step 2–3 (all answered)

- ~~Path or git URL of the Filer Desktop repo to vendor from (and the commit to pin).~~ **Answered 2026-11-02:** document moves are based on the most recent Vibe Filer Desktop repo — `KisaesDevLab/Vibe-Filer-Desktop` @ `master` `ea4e7267797ee869f05c3e5745700e13910a2e5b` (v0.2.4, 2026-11-02). Re-check `master` at P00 step 2 and take a newer HEAD if one exists.
- ~~Where the Practice Manager Filer module source lives.~~ **Answered 2026-11-02:** ignore Practice Manager Filer; use Filer Desktop. `docs/SENTINEL_V1.md` comes from Filer Desktop's `sentinel.rs`; `docs/BUCKET_LAYOUT.md` is Vibe Files' own layout mirroring the Filer Desktop tree.

## Phase log

| Phase | Status | Started | Done | Notes |
|---|---|---|---|---|
| P00 | done | 2026-11-02 | 2026-11-02 | last box confirmed on GitHub windows-latest (CI run 41506666554) |
| P01 | built; owner UI check pending | 2026-11-02 | | all 8 steps; Done-when covered by tests |
| P02 | pending | | | |
| P03 | pending | | | |
| P04 | pending | | | |
| P05 | pending | | | |
| P06 | pending | | | |
| P07 | pending | | | |
| P08 | pending | | | |
| P09 | pending | | | |
| P10 | pending | | | |
| P11 | pending | | | |
| P12 | pending | | | |
| P13 | pending | | | |
| PA-12 | done (merge pending) | 2026-11-02 | 2026-11-02 | CI + release workflows green on PR #1 |

## Assumptions taken (from QUESTIONS defaults)

- 2026-11-02 (P01): the catalog key is random and wrapped by the recovery passphrase (`<data>/keys.json`); P06 adds a DPAPI wrapping of the same key. One data folder holds one cabinet's catalog.
- 2026-11-02 (P01): the cabinet's folder template is `<cabinet>/_Vibe/template.json` (Filer format, seeded with Filer's default), so every workstation renders the same tree.
- 2026-11-02 (P01): Filer's relationship format has no share field; a share is stored at the end of the note as `[share 40%]`.
- 2026-11-02 (P01): "Needs tax year" = no tax year, in a client folder, outside the template's `Permanent` folder. "Recent" = this user's last 50 opens.
- 2026-11-02 (owner): UI is not tested under Wine; the owner runs UI checks on a Windows PC.

- 2026-11-02 (owner): move/copy/collision/undo behavior comes from Vibe Filer Desktop `KisaesDevLab/Vibe-Filer-Desktop` @ `master` `ea4e7267797ee869f05c3e5745700e13910a2e5b` (v0.2.4, 2026-11-02). See CLAUDE.md §Document moves.
- Upstream is one binary crate; P00 step 2 is a mechanical split into `filer-core` / `filer-pdf` / `filer-intake` (Q-16 default).
- Workspace is edition 2024 and egui 0.37 to match the vendored code.
- 2026-11-02 (owner): Practice Manager Filer is ignored. No PM layout, no PM-held key, no portal pull in v1 (moved to PA-11). Q-01, Q-03, Q-11 closed; Q-04 reworded.

## Known issues / debt

- **Metadata lives only in the catalog** (P01): doc type, status, source, tags, entity type, pins, favorites, recent opens. `rebuild` restores clients, documents, versions and relationships (mirrored to `_Vibe/relationships.json`), not these. Needs an on-disk metadata sidecar before real use beyond one PC — propose as an addendum (PA-13) or fold into P09/P11.
- CSV import binds existing folders only by the template's exact client-folder name. Filer's fuzzy folder binding (`sentinel::scan_root`) belongs with the Conflicts work.
- Undo last = the last transaction of this session only; the per-user 20-deep stack is P05.
- The catalog asks for the recovery passphrase on every launch until P06 adds the DPAPI wrapping.
- Double-click opens PDFs in the registered app until the Viewer tab (P04).
- Rename refuses a letter-case-only change (Filer's no-replace move sees the same file).
- The tree-expansion timing test runs on 5,000 catalog rows (50 clients), not 5,000 files on disk.
- egui_kittest cannot render screenshots under Wine (no GPU adapter, even with lavapipe).

- Wine-only failure (not ours): `filer-intake` `app::plan_ui_tests::twenty_file_batch_resolves_to_ready` usually fails under Wine — the plan ends with fewer rows than the queue (inbox watcher events under Wine's change-notification emulation). Same in the untouched upstream repo; passes even under Wine some runs. `scripts/check-wine.sh` warns instead of failing for this one test. **Must be confirmed green on real Windows** before P00 is closed.
- Upstream flake (not ours): `filer-intake` `app::combine_ui_tests::rename_change_folder_and_change_client` and three other `*_ui_tests` wait a fixed 5 s (`settle`, 500 × 10 ms) for background work; under Wine they sometimes time out and pass with a longer budget (checked with a temporary, uncommitted 60 s budget). The gate re-runs a failed one once. Fails the same way in the untouched upstream repo. Not edited (vendored); report upstream.
- CLI key source until P06: `--key-file` / `VF_CATALOG_KEY` (raw 64-hex catalog key). Replace with DPAPI + recovery passphrase in P06.
- `device_id` is stored in `<data>/device_id` until `config.toml` lands (P01); move it then.
- Release builds use the Windows GUI subsystem, so CLI subcommands print nothing in a release exe. Attach to the parent console (or ship a console twin) in P13 step 5.
- `rebuild` takes a document's tax year from the first 4-digit year folder in its path (Filer `{Year}` folders); doc type is left empty. Both are re-derived properly once doc types (P01) and extraction (P08) exist.
- Dev box gate is `scripts/check-wine.sh` (mingw cross-build + Wine). Real Windows runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Performance targets (record actuals in P13)

| Metric | Target | Actual |
|---|---|---|
| Cold start to Browse | < 2 s | |
| Tree expand @ 50k docs | < 100 ms | |
| FTS query @ 50k docs | < 100 ms | |
| First page render, 300-page scan | < 200 ms | |
| Remote open @ 20 Mbps | < 3 s | |
| LAN soak (4 writers, 10 min) | 0 corruption | |
