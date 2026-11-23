# Releasing Vibe Files

This repository (`KisaesDevLab/Vibe-Files`) holds the source and is meant to be **private**.
Releases are built on the Kisaes build PC and published, **binary only**, to the public release
repository **`KisaesDevLab/vibe-files-releases`** (local checkout `C:\Projects\vibe-files-releases`,
a separate repo beside this one: not a submodule and not a second remote). No GitHub Actions.

Stable links (the website, the README and the app use these):

- `https://github.com/KisaesDevLab/vibe-files-releases/releases/latest/download/vibe-files-windows-x64.zip`
- `https://github.com/KisaesDevLab/vibe-files-releases/releases/latest/download/latest.json`
- the app's update check: `api.github.com/repos/KisaesDevLab/vibe-files-releases/releases/latest`
  (`crates/vf-sync/src/update.rs`)

"latest" skips pre-releases, so releases are published as **full releases** (owner 2026-11-22).

## Prerequisites (build PC)

- `gh` signed in as an account that can create releases on `vibe-files-releases`
  (`gh auth status`).
- Rust stable with `rustup target add i686-pc-windows-msvc` (the 32-bit scanning helper).
- Inno Setup 6 (`winget install --id JRSoftware.InnoSetup -e`; `ISCC.exe` is found on PATH, in
  `INNO_ISCC`, or in its default folders) for the installer.
- Tesseract 5.4 at `C:\Program Files\Tesseract-OCR` (or pass `-Tesseract <dir>`), and Python
  (for `scripts/dll_closure.py`).
- `C:\Projects\vibe-files-releases` checked out, clean, with `origin` set.
- Optional code signing: set `VF_SIGN_CERT_THUMBPRINT` to the SHA-1 thumbprint of a code-signing
  certificate in the Windows certificate store (and `VF_SIGN_TIMESTAMP_URL` to override the
  timestamp server). Without it the build ships unsigned and the script says so; SmartScreen then
  warns users on first run.

## One-command release

1. Bump `version` in `Cargo.toml` (`[workspace.package]`), run `cargo build` once so `Cargo.lock`
   picks up the new version, and commit **both** files (the script builds with `--locked` and
   stops if `Cargo.lock` is out of date).
2. Releases are cut from **`main`**. Merge or fast-forward the work onto `main` and check it out.
3. In `C:\Projects\vibe-files-releases`, add a `## <version> (<date>)` section at the top of
   `CHANGELOG.md`, written for users, and commit it (don't push; the script does). That section
   becomes the release notes.
4. Dry run, and read its output:

   ```powershell
   pwsh tools/release.ps1 -DryRun
   ```

   It runs the tests, builds, packages, generates `THIRD-PARTY-NOTICES.txt`, runs
   `tools/check-binary.ps1` on everything that ships, writes the assets to
   `target/dist/v<version>/`, and prints the exact publish commands without running them. Checks
   that only matter for publishing (branch, existing tag, `gh` sign-in, the public repo's remote)
   are listed as "a real run would stop" instead of stopping the dry run. A dirty tree or a
   missing changelog section stops it before building.
5. Smoke-test `target/dist/v<version>/vibe-files-windows-x64.zip` on a clean PC: open a cabinet,
   view a PDF, OCR one scan, push.
6. Release for real:

   ```powershell
   pwsh tools/release.ps1
   ```

   It repeats every check (a remote it cannot reach stops it; it never counts as "no tag" or
   "no release"; the public checkout must be on `main` and not behind origin). Then it tags
   `v<version>` here and tries the tag's push with `--dry-run`, pushes the public repo's
   changelog commit to `main`, runs
   `gh release create v<version> --repo KisaesDevLab/vibe-files-releases --target <that commit> --latest`
   with the assets, and pushes the tag. A failure before the release exists removes the local
   tag; a failure after says to push the tag by hand. It prints the download URLs and every
   asset's SHA-256.
7. winget (optional): open a pull request to `microsoft/winget-pkgs` with
   `target/dist/v<version>/winget/manifests/k/Kisaes/VibeFiles/<version>/`. The installer URL in
   those manifests is the per-version link, never "latest".

Other switches: `-SkipTests`; `-Version <x.y.z>` (must equal `Cargo.toml`); `-AllowDirty`
(only with `-DryRun`: a trial build of uncommitted work, marked `TRIAL.txt`, never published);
`-PubDir <path>`; `-Tesseract <dir>`; `-TestJobs <n>` and `-BuildJobs <n>` (both default 2).

### Memory and flaky tests on the build PC

A release build is heavy: the fat-LTO link of `vibe-move.exe` alone peaked at 7.1 GB, and rustc
crashed with "out of memory" / `STATUS_STACK_BUFFER_OVERRUN` on this 16 GB PC at the default job
count and at `-j 4` (WSL was holding about 8 GB). The script links one program per cargo run at
`-BuildJobs 2`. Before a real release, close WSL (`wsl --shutdown`) and other builds. If it still
crashes, rerun with `-BuildJobs 1`: finished crates are kept, so a rerun only redoes what failed.

Two timing tests can fail under the full parallel test run and pass alone:
`vf-app ui::browse::tests::tree_expands_fast_at_5000_documents` and
`filer-intake app::combine_ui_tests::ticked_rows_commit_alone`. If the test step stops on one of
these, run it alone (`cargo test -p <crate> <name>`). If it passes, run the rest with
`cargo test --workspace --no-fail-fast -j 2`, and when only those flakes fail, release with
`-SkipTests`. Any other failure stops the release.

### Assets (fixed names, so "latest" links never change)

| Asset | Contents |
|---|---|
| `vibe-files-setup-x64.exe` | the installer (`installer/VibeFiles.iss`, PA-102) made from the same staged files: asks **for me only** (`%LOCALAPPDATA%\Programs\Vibe Files`, no administrator) or **for all users** (`%ProgramFiles%\Vibe Files`); silent `/VERYSILENT /CURRENTUSER` or `/ALLUSERS`. It adds `installed.txt` beside the exe, so every login's data is in `%LOCALAPPDATA%\Vibe Files\data`. Upgrade and uninstall never touch the data, the cabinet or a `firm.toml` put beside the exe. Never change its `AppId`. |
| `vibe-files-windows-x64.zip` | the portable folder: `vibe-files.exe`, `vibe-move.exe`, `vibe-scan32.exe`, `pdfium.dll`, `TWAINDSM.dll`, `tesseract\`, `LICENSE.txt`, `NOTICE.txt`, `THIRD-PARTY-NOTICES.txt`, `OPERATIONS.md`, third-party licence files |
| `SHA256SUMS.txt` | SHA-256 of each asset (the installer too) and of the three exes inside the zip |
| `latest.json` | `{ version, released_at, notes_url, assets: [{ name, sha256, size }] }` |
| anything from `-ExtraAssets` | copied in as-is, hashed, listed in `latest.json` |

### macOS or Linux artifacts

Vibe Files is Windows-only today. If another platform's build is made on another machine, put
the finished files (final names, for example `vibe-files-macos-arm64.zip`) in one folder and
pass it: `pwsh tools/release.ps1 -ExtraAssets D:\other-builds`. They are scanned by
`check-binary.ps1`, hashed into `SHA256SUMS.txt`, listed in `latest.json` and attached to the
release.

### What the build does to keep local details out

- `[profile.release]`: `debug = false`, `strip = true`, `lto = true`, `codegen-units = 1`,
  `opt-level = "z"`.
- `--remap-path-prefix` for the user profile, `C:\Projects`, the rustup and cargo homes, the
  target folder, the shared `build-dir` (`C:\Projects\cl-target\<hash>`) and this repo, passed
  through `CARGO_ENCODED_RUSTFLAGS` with `+crt-static`.
- `VF_COMMIT` (the short commit hash) is set for the build and shown in the footer as
  `vibe-files <version> (<commit>)`: a build identifier, never a link.
- `tools/check-binary.ps1 <file|folder|zip>` fails on: this PC's Windows user name, `C:\Projects`
  or `C:\Users\<name>\` paths, any `KisaesDevLab/<repo>` other than `vibe-files-releases`, and
  GitHub/AWS/Slack/API token and private-key patterns. Run it by hand on anything you are about
  to share.
- What stays readable after packaging: `OPERATIONS.md`, the licence texts and the Tesseract data
  are plain files. The Rust exes carry no source, but their embedded strings (help articles, UI
  text, SQL schema and migrations, the default folder template, and crate names in panic
  messages) can be read with any strings tool. There is no Node or other script bundle.

### Symbols

The release profile writes no `.pdb`. If one is made (for example after turning debug info back
on to chase a crash), the script copies it to `dist/symbols/<version>/` in this repo, which git
ignores. Symbols are never shipped.

## Pulling a bad release

1. Unpublish it so "latest" falls back to the previous release:
   `gh release delete v<version> --repo KisaesDevLab/vibe-files-releases --cleanup-tag --yes`
2. Remove the tag here: `git tag -d v<version>` and `git push origin :refs/tags/v<version>`.
3. Prefer fixing forward with a **new** version number (bump `Cargo.toml`, add a changelog
   section that says what was wrong with the pulled one, release again). Re-using the same number
   is possible after steps 1 and 2 (`tools/release.ps1` then finds no tag), but anyone who already
   downloaded it, and winget if it was submitted, holds different bytes under the same version.
4. If a winget pull request was merged for it, open one that removes or supersedes that version.

## Owner cutover checklist (one time; the owner does these, not the agent)

Prepared in PA-100. Done so far: step 0's changelog date and step 2's fast-forward and bump (0.1.52,
2026-11-23). No repository visibility was changed and nothing was published.

0. **Licence (PA-101).** `LICENSE.md` is the VibeMyFirm Community License 1.0, the Kisaes
   licensing package's text byte for byte; the public repo's `LICENSE.md` carries the same text
   under a short list of the program's parts. `NOTICE.md` (installed as `NOTICE.txt`) and the
   keyless-Professional-Use permission were drafted here after ClientLedger Trial Balance's
   (its Q145) and are **not counsel-reviewed**: have counsel read them with the licence before
   1.0. If `LICENSE.md` ever changes, change both copies. Date the `0.1.52 (unreleased)` heading
   in the public `CHANGELOG.md`.

1. On GitHub, create **`KisaesDevLab/vibe-files-releases`**, **public and empty** (no README,
   licence or .gitignore: the local checkout has them). In
   `C:\Projects\vibe-files-releases`: `git remote add origin https://github.com/KisaesDevLab/vibe-files-releases.git`
   and `git push -u origin main`. In its settings, keep **Issues** on (the issue forms
   turn blank issues off and send client data and security reports to support@kisaes.com).
2. Fast-forward `main` in this repo to the release branch, and bump the version (0.1.52 is the
   first one under the new licence; its changelog section is drafted). Then run
   `pwsh tools/release.ps1 -DryRun`, read it, and run `pwsh tools/release.ps1`.
3. In a browser that is **not signed in** to GitHub, download
   `https://github.com/KisaesDevLab/vibe-files-releases/releases/latest/download/vibe-files-windows-x64.zip`
   and `SHA256SUMS.txt` from the same release, and check them:
   `Get-FileHash .\vibe-files-windows-x64.zip` must match.
4. Existing releases on `KisaesDevLab/Vibe-Files` (0.1.x, all pre-releases) stop being downloadable
   once the repo is private. **Recommended: let them go.** Their exes were built before PA-100 and
   contain this PC's Windows user name, local build paths and the dev repo's name (the PA-100
   baseline `tools/check-binary.ps1` run on 0.1.51 found 10 hits), and they carry the AGPL licence.
   If one is still needed, rebuild it from its tag with `tools/release.ps1` instead of re-uploading
   the old zip.
5. Builds 0.1.51 and earlier check `KisaesDevLab/Vibe-Files` for updates. Once that repo is
   private they will never report a newer version. Tell every office to download 0.1.52 or later
   by hand once; from then on the update check uses the public repo.
6. Update download links on the product website to the stable `latest/download/...` URL.
7. Make `KisaesDevLab/Vibe-Files` **private** (Settings › General › Danger Zone). Expect stars and
   watchers to be dropped and public forks to be detached.
8. From the signed-out browser: `https://github.com/KisaesDevLab/Vibe-Files` must show 404, and
   the step 3 download must still work.

## Third-party notices

`tools/third-party-notices.ps1` copies each crate's own licence files (from a subfolder when the
crate keeps them there, as egui's font crate does). For the 18 crates that name a licence but
publish no file of it (the egui family, accesskit, clipboard-win, enum-map, profiling, spirv), it
appends the standard text of that licence (MIT, Apache-2.0 or BSL-1.0) with the crates' authors
as the copyright line (Q-143, as ClientLedger Trial Balance does). A crate whose licence has no
standard text in the script stops the release until one is added.
