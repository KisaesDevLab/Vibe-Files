# Portable release of Vibe Files (PHASES P13 step 6; QUESTIONS Q-30..Q-34).
#
#   pwsh scripts/release.ps1            # build, stage, zip, hash, winget manifests
#   pwsh scripts/release.ps1 -SkipBuild # reuse the last release build (a trial: not for publishing)
#
# Output in <target>/dist/:
#   vibe-files-<ver>-win64/      the portable folder: vibe-files.exe, vibe-move.exe and the 32-bit
#                                scanning helper vibe-scan32.exe (static CRT), pdfium.dll,
#                                TWAINDSM.dll, tesseract\ (English), licenses
#   vibe-files-<ver>-win64.zip   what the GitHub release and winget serve
#   SHA256SUMS.txt               SHA-256 of the zip and of the exe (published with the release)
#   winget/manifests/k/Kisaes/VibeFiles/<ver>/*.yaml   for a microsoft/winget-pkgs pull request
#
# Publishing (the GitHub release, the winget-pkgs pull request) is a separate, deliberate step:
# see docs/OPERATIONS.md §Release. This script changes nothing outside target/.
param(
    [switch]$SkipBuild,
    # Allow uncommitted changes (a trial build only: never publish one).
    [switch]$AllowDirty,
    [string]$Tesseract = 'C:\Program Files\Tesseract-OCR'
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot
Set-Location $repo

# A release is built from a committed tree, so the zip matches a commit (QA X11).
$dirty = git status --porcelain
if ($dirty -and -not $AllowDirty) {
    throw "uncommitted changes; commit them first (or -AllowDirty for a trial build):`n$($dirty -join "`n")"
}
$commit = (git rev-parse HEAD).Trim()
# The version of the program itself (vf-app), as cargo resolves it.
$meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
$ver = ($meta.packages | Where-Object name -eq 'vf-app').version
if (-not $ver) { throw 'no vf-app package version' }
if (-not $SkipBuild) {
    # Static C runtime: the exe runs on a PC without the VC++ redistributable.
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    # --locked: the build never rewrites Cargo.lock after the clean-tree check (QA2 #8).
    try {
        cargo build --release --locked -p vf-app -p vf-move; if ($LASTEXITCODE) { throw 'build failed' }
        # The 32-bit scanning helper (Citrix / Remote Desktop scanning reaches 32-bit programs
        # only): needs `rustup target add i686-pc-windows-msvc`.
        cargo build --release --locked -p vf-scan32 --target i686-pc-windows-msvc
        if ($LASTEXITCODE) { throw 'helper build failed' }
    }
    finally { Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue }
}
$target = $meta.target_directory
$exe = Join-Path $target 'release\vibe-files.exe'
if (-not (Test-Path $exe)) { throw "no release build at $exe" }
# The tree-splitting utility (PA-35), shipped beside the program.
$moveExe = Join-Path $target 'release\vibe-move.exe'
if (-not (Test-Path $moveExe)) { throw "no release build at $moveExe" }
$scanExe = Join-Path $target 'i686-pc-windows-msvc\release\vibe-scan32.exe'
if (-not (Test-Path $scanExe)) { throw "no 32-bit helper build at $scanExe" }

$name = "vibe-files-$ver-win64"
$dist = Join-Path $target 'dist'
$stage = Join-Path $dist $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item $exe $stage
Copy-Item $moveExe $stage
Copy-Item $scanExe $stage
Copy-Item crates\filer-pdf\assets\pdfium.dll, crates\filer-pdf\assets\pdfium-LICENSE $stage
if (Test-Path crates\filer-pdf\assets\pdfium-licenses) {
    Copy-Item -Recurse crates\filer-pdf\assets\pdfium-licenses (Join-Path $stage 'pdfium-licenses')
}
Copy-Item crates\vf-ingest\assets\TWAINDSM.dll, crates\vf-ingest\assets\TWAINDSM-LICENSE.txt $stage
Copy-Item LICENSE $stage
Copy-Item docs\OPERATIONS.md $stage

# Tesseract (Q-25, Q-32): the program, only the DLLs it loads, English + script detection data.
$t = Join-Path $stage 'tesseract'
New-Item -ItemType Directory -Force (Join-Path $t 'tessdata') | Out-Null
Copy-Item (Join-Path $Tesseract 'tesseract.exe') $t
$dlls = python scripts\dll_closure.py $Tesseract tesseract.exe
if ($LASTEXITCODE) { throw 'could not read the Tesseract DLL list' }
foreach ($d in $dlls) { Copy-Item (Join-Path $Tesseract $d) $t }
foreach ($f in 'eng.traineddata', 'osd.traineddata') {
    Copy-Item (Join-Path $Tesseract "tessdata\$f") (Join-Path $t 'tessdata')
}
Copy-Item (Join-Path $Tesseract 'doc\LICENSE') (Join-Path $t 'LICENSE')
& (Join-Path $t 'tesseract.exe') --version | Select-Object -First 1

$zip = Join-Path $dist "$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
$zipHash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
$exeHash = (Get-FileHash (Join-Path $stage 'vibe-files.exe') -Algorithm SHA256).Hash.ToLower()
@(
    "$zipHash  $name.zip",
    "$exeHash  $name/vibe-files.exe",
    "$((Get-FileHash (Join-Path $stage 'vibe-move.exe') -Algorithm SHA256).Hash.ToLower())  $name/vibe-move.exe"
    "$((Get-FileHash (Join-Path $stage 'vibe-scan32.exe') -Algorithm SHA256).Hash.ToLower())  $name/vibe-scan32.exe"
) | Set-Content -Encoding ascii (Join-Path $dist 'SHA256SUMS.txt')

# winget manifests (portable zip; the data folder then lives in %LOCALAPPDATA%\Vibe Files).
$w = Join-Path $dist "winget\manifests\k\Kisaes\VibeFiles\$ver"
New-Item -ItemType Directory -Force $w | Out-Null
$url = "https://github.com/KisaesDevLab/Vibe-Files/releases/download/v$ver/$name.zip"
@"
PackageIdentifier: Kisaes.VibeFiles
PackageVersion: $ver
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $w 'Kisaes.VibeFiles.yaml')
@"
PackageIdentifier: Kisaes.VibeFiles
PackageVersion: $ver
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
  - RelativeFilePath: vibe-files.exe
    PortableCommandAlias: vibe-files
  - RelativeFilePath: vibe-move.exe
    PortableCommandAlias: vibe-move
ArchiveBinariesDependOnPath: true
Installers:
  - Architecture: x64
    InstallerUrl: $url
    InstallerSha256: $($zipHash.ToUpper())
ManifestType: installer
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $w 'Kisaes.VibeFiles.installer.yaml')
@"
PackageIdentifier: Kisaes.VibeFiles
PackageVersion: $ver
PackageLocale: en-US
Publisher: Kisaes LLC
PublisherUrl: https://github.com/KisaesDevLab
PackageName: Vibe Files
PackageUrl: https://github.com/KisaesDevLab/Vibe-Files
License: AGPL-3.0-or-later
LicenseUrl: https://github.com/KisaesDevLab/Vibe-Files/blob/main/LICENSE
ShortDescription: Document management for small CPA and tax firms over a plain client folder tree.
Tags:
  - document-management
  - accounting
  - tax
ManifestType: defaultLocale
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $w 'Kisaes.VibeFiles.locale.en-US.yaml')

$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
# The exe must come from this commit: a reused build (-SkipBuild) or changes left after the build
# make it a trial (QA2 #8).
$after = git status --porcelain
$trial = if ($dirty -or $after) { ' (UNCOMMITTED CHANGES: trial only)' } elseif ($SkipBuild) { ' (REUSED BUILD, not checked against the commit: trial only)' } else { '' }
# A trial leaves a marker beside the zip, so it is never taken for a release (QA3).
$marker = Join-Path $dist 'TRIAL.txt'
if ($trial) { "Trial build$trial from $commit; do not publish." | Set-Content -Encoding ascii $marker }
elseif (Test-Path $marker) { Remove-Item $marker }
"Release $ver staged from $commit${trial}: $zip ($mb MB)"
"SHA-256 zip $zipHash"
"SHA-256 exe $exeHash"
"winget manifests: $w"
