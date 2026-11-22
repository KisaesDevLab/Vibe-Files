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
- Tesseract 5.4 at `C:\Program Files\Tesseract-OCR` (or pass `-Tesseract <dir>`), and Python
  (for `scripts/dll_closure.py`).
- `C:\Projects\vibe-files-releases` checked out, clean, with `origin` set.
- Optional code signing: set `VF_SIGN_CERT_THUMBPRINT` to the SHA-1 thumbprint of a code-signing
  certificate in the Windows certificate store (and `VF_SIGN_TIMESTAMP_URL` to override the
  timestamp server). Without it the build ships unsigned and the script says so; SmartScreen then
  warns users on first run.

## One-command release

1. Bump `version` in `Cargo.toml` (`[workspace.package]`) and commit.
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

   It repeats every check, then pushes the public repo's changelog commit, runs
   `gh release create v<version> --repo KisaesDevLab/vibe-files-releases --latest` with the
   assets, and tags `v<version>` here and pushes the tag. It prints the download URLs and every
   asset's SHA-256.
7. winget (optional): open a pull request to `microsoft/winget-pkgs` with
   `target/dist/v<version>/winget/manifests/k/Kisaes/VibeFiles/<version>/`. The installer URL in
   those manifests is the per-version link, never "latest".

Other switches: `-SkipTests`; `-Version <x.y.z>` (must equal `Cargo.toml`); `-AllowDirty`
(only with `-DryRun`: a trial build of uncommitted work, marked `TRIAL.txt`, never published);
`-PubDir <path>`; `-Tesseract <dir>`; `-TestJobs <n>` (default 2: `-j 4` crashed rustc once
on this PC).

### Assets (fixed names, so "latest" links never change)

| Asset | Contents |
|---|---|
| `vibe-files-windows-x64.zip` | the portable folder: `vibe-files.exe`, `vibe-move.exe`, `vibe-scan32.exe`, `pdfium.dll`, `TWAINDSM.dll`, `tesseract\`, `LICENSE`, `THIRD-PARTY-NOTICES.txt`, `OPERATIONS.md`, third-party licence files |
| `SHA256SUMS.txt` | SHA-256 of each asset and of the three exes inside the zip |
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

Prepared in PA-100. Nothing below has been done yet. No repository visibility was changed.

1. On GitHub, create **`KisaesDevLab/vibe-files-releases`**, **public and empty** (no README,
   licence or .gitignore: the local checkout has them). In
   `C:\Projects\vibe-files-releases`: `git remote add origin https://github.com/KisaesDevLab/vibe-files-releases.git`
   and `git push -u origin main`. In its settings, turn on **Issues** and **Private
   vulnerability reporting** (Security › Advisories; `SECURITY.md` relies on it).
2. Fast-forward `main` in this repo to the release branch, and bump the version (0.1.52 is the
   first one under the new licence; its changelog section is drafted). Then run
   `pwsh tools/release.ps1 -DryRun`, read it, and run `pwsh tools/release.ps1`.
3. In a browser that is **not signed in** to GitHub, download
   `https://github.com/KisaesDevLab/vibe-files-releases/releases/latest/download/vibe-files-windows-x64.zip`
   and `SHA256SUMS.txt` from the same release, and check them:
   `Get-FileHash .\vibe-files-windows-x64.zip` must match.
4. Move the existing releases off `KisaesDevLab/Vibe-Files` (0.1.x, all pre-releases): once the
   repo is private they can't be downloaded by anyone outside. Either upload the ones still needed
   to the public repo (`gh release download v0.1.51 --repo KisaesDevLab/Vibe-Files` then
   `gh release create v0.1.51 --repo KisaesDevLab/vibe-files-releases --prerelease ...`; keep
   them pre-release so "latest" stays on 0.1.52+, and note that their zips carry the AGPL licence
   they were published under) or accept that they go away.
5. Builds 0.1.51 and earlier check `KisaesDevLab/Vibe-Files` for updates. Once that repo is
   private they will never report a newer version. Tell every office to download 0.1.52 or later
   by hand once; from then on the update check uses the public repo.
6. Update download links on the product website to the stable `latest/download/...` URL.
7. Make `KisaesDevLab/Vibe-Files` **private** (Settings › General › Danger Zone). Expect stars and
   watchers to be dropped and public forks to be detached.
8. From the signed-out browser: `https://github.com/KisaesDevLab/Vibe-Files` must show 404, and
   the step 3 download must still work.
9. Have the draft `LICENSE` (end user licence agreement) reviewed. Fill in the governing-law
   state, remove the DRAFT notice, and copy the final text into the public repo's `LICENSE.md`.
   Do this before step 2 if the first public release should carry the final text.
