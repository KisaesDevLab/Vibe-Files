# Vibe Files — STATE.md

Updated by Claude Code at the end of every session. Human edits welcome.

## Current phase

**P00 — Workspace, vendored Filer crates, sync-aware catalog schema** — all 8 steps built (2026-11-02). One Done-when item waits on a real Windows run (see Next step).

## Next step

1. On a Windows PC: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace -- --test-threads=2`. Confirm `filer-intake` `app::plan_ui_tests::twenty_file_batch_resolves_to_ready` passes there (it fails only under Wine; see debt). Then tick the last P00 box and mark P00 done.
2. Start P01 step 1: cabinet open/create wizard (pick root, name, recovery passphrase, write `_Vibe/cabinet.json` via `vf_fs::cabinet::create`, run `vf_fs::rebuild`).

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

## Prerequisites before P00 step 2–3 (all answered)

- ~~Path or git URL of the Filer Desktop repo to vendor from (and the commit to pin).~~ **Answered 2026-11-02:** document moves are based on the most recent Vibe Filer Desktop repo — `KisaesDevLab/Vibe-Filer-Desktop` @ `master` `ea4e7267797ee869f05c3e5745700e13910a2e5b` (v0.2.4, 2026-11-02). Re-check `master` at P00 step 2 and take a newer HEAD if one exists.
- ~~Where the Practice Manager Filer module source lives.~~ **Answered 2026-11-02:** ignore Practice Manager Filer; use Filer Desktop. `docs/SENTINEL_V1.md` comes from Filer Desktop's `sentinel.rs`; `docs/BUCKET_LAYOUT.md` is Vibe Files' own layout mirroring the Filer Desktop tree.

## Phase log

| Phase | Status | Started | Done | Notes |
|---|---|---|---|---|
| P00 | built; Windows check pending | 2026-11-02 | | all steps committed; one Wine-only upstream UI test to confirm on Windows |
| P01 | pending | | | |
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

## Assumptions taken (from QUESTIONS defaults)

- 2026-11-02 (owner): move/copy/collision/undo behavior comes from Vibe Filer Desktop `KisaesDevLab/Vibe-Filer-Desktop` @ `master` `ea4e7267797ee869f05c3e5745700e13910a2e5b` (v0.2.4, 2026-11-02). See CLAUDE.md §Document moves.
- Upstream is one binary crate; P00 step 2 is a mechanical split into `filer-core` / `filer-pdf` / `filer-intake` (Q-16 default).
- Workspace is edition 2024 and egui 0.37 to match the vendored code.
- 2026-11-02 (owner): Practice Manager Filer is ignored. No PM layout, no PM-held key, no portal pull in v1 (moved to PA-11). Q-01, Q-03, Q-11 closed; Q-04 reworded.

## Known issues / debt

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
