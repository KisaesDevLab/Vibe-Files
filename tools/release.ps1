#requires -Version 7
# One-command release of Vibe Files (PA-100 R3; first written for P13 step 6, QUESTIONS Q-30..Q-34).
# Builds from this private repo and publishes the binary only to the public release repo
# KisaesDevLab/vibe-files-releases. See RELEASING.md.
#
#   pwsh tools/release.ps1 -DryRun              # everything but publishing; prints the publish commands
#   pwsh tools/release.ps1                      # the real release (the owner runs this)
#   pwsh tools/release.ps1 -DryRun -AllowDirty  # a trial build of uncommitted work (never published)
#
# Output in <target>/dist/v<ver>/:
#   stage/                      the portable folder: vibe-files.exe, vibe-move.exe, vibe-scan32.exe
#                               (static CRT), pdfium.dll, TWAINDSM.dll, tesseract\ (English), licences
#   vibe-files-windows-x64.zip  the release asset (fixed name, so the "latest" link never changes)
#   SHA256SUMS.txt              SHA-256 of every asset (and of the exes inside the zip)
#   latest.json                 { version, released_at, notes_url, assets: [{ name, sha256, size }] }
#   notes.md                    this version's CHANGELOG.md section (the release notes)
#   winget/manifests/k/Kisaes/VibeFiles/<ver>/*.yaml   for a microsoft/winget-pkgs pull request
# Symbols (.pdb, if the build made any) go to dist/symbols/<ver>/ in this repo (ignored by git).
param(
    # Defaults to the program's version in Cargo.toml; a different value is refused.
    [string]$Version,
    # Build, package and check, but publish nothing: print the commands a real run would use.
    [switch]$DryRun,
    [switch]$SkipTests,
    # Artifacts built on another machine (macOS, Linux): every file in it is added as an asset.
    [string]$ExtraAssets,
    # Allow uncommitted changes. Only with -DryRun: a trial build, never published.
    [switch]$AllowDirty,
    [string]$PubDir = (Join-Path (Split-Path (Split-Path $PSScriptRoot)) 'vibe-files-releases'),
    [string]$Tesseract = 'C:\Program Files\Tesseract-OCR',
    # Parallel test jobs (-j 4 crashed rustc once on this PC: STATE.md PA-99).
    [int]$TestJobs = 2,
    # Parallel build jobs: the default (one per core), and 4 under load, ran this 16 GB PC out of memory.
    [int]$BuildJobs = 2
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

$App = 'Vibe Files'
$PubRepo = 'KisaesDevLab/vibe-files-releases'
$ZipName = 'vibe-files-windows-x64.zip'
$DefaultBranch = 'main'

$repo = Split-Path $PSScriptRoot
Set-Location $repo

function Stop-Release([string]$why) {
    Write-Host "release stopped: $why" -ForegroundColor Red
    exit 1
}
function Step([string]$what) { Write-Host "`n== $what" -ForegroundColor Cyan }
# A check that a real run must pass but a dry run only reports (it tests the build, not the
# publish): an existing tag, the branch, gh sign-in, the public repo's remote.
$warned = @()
function Require-ForPublish([bool]$ok, [string]$why) {
    if ($ok) { return }
    if ($DryRun) { $script:warned += $why; Write-Host "  (dry run) a real run would stop: $why" -ForegroundColor Yellow }
    else { Stop-Release $why }
}

# ---------------------------------------------------------------- 1. preflight
Step 'Preflight'
if ($AllowDirty -and -not $DryRun) { Stop-Release '-AllowDirty is only for -DryRun trial builds' }
$dirty = git status --porcelain
if ($dirty -and -not $AllowDirty) {
    Stop-Release "uncommitted changes in $repo; commit them first:`n$($dirty -join "`n")"
}
$meta = cargo metadata --format-version 1 --no-deps --locked | ConvertFrom-Json
if ($LASTEXITCODE) { Stop-Release 'cargo metadata failed (is Cargo.lock up to date?)' }
$cargoVer = ($meta.packages | Where-Object name -eq 'vf-app').version
if (-not $cargoVer) { Stop-Release 'no vf-app package version' }
if (-not $Version) { $Version = $cargoVer }
$Version = $Version.TrimStart('v')
if ($Version -ne $cargoVer) { Stop-Release "-Version $Version is not the Cargo.toml version $cargoVer; bump Cargo.toml instead" }
$tag = "v$Version"
$commit = (git rev-parse HEAD).Trim()
$short = (git rev-parse --short=9 HEAD).Trim()
$branch = (git branch --show-current).Trim()
"  version $Version, commit $short, branch $branch"

Require-ForPublish ($branch -eq $DefaultBranch) "on branch '$branch', not $DefaultBranch"
Require-ForPublish (-not (git tag -l $tag)) "tag $tag already exists in this repo"
$remoteTag = git ls-remote --tags origin "refs/tags/$tag" 2>$null
Require-ForPublish (-not $remoteTag) "tag $tag already exists on origin"
$null = gh auth status 2>&1
$ghOk = ($LASTEXITCODE -eq 0)
Require-ForPublish $ghOk 'gh is not signed in (gh auth login)'
if ($ghOk) {
    $null = gh release view $tag --repo $PubRepo 2>&1
    Require-ForPublish ($LASTEXITCODE -ne 0) "release $tag already exists on $PubRepo (or $PubRepo is unreachable)"
}

if (-not (Test-Path (Join-Path $PubDir '.git'))) { Stop-Release "no public release repo checkout at $PubDir" }
$pubDirty = git -C $PubDir status --porcelain
if ($pubDirty) { Stop-Release "uncommitted changes in $PubDir; commit the CHANGELOG.md section first:`n$($pubDirty -join "`n")" }
$pubOrigin = git -C $PubDir remote get-url origin 2>$null
Require-ForPublish ([bool]$pubOrigin) "$PubDir has no origin remote (RELEASING.md owner step 1)"
$pubTag = git -C $PubDir tag -l $tag
Require-ForPublish (-not $pubTag) "tag $tag already exists in $PubDir"

# This version's CHANGELOG.md section: from "## 0.1.52" (or "## [0.1.52]", "## v0.1.52", with
# anything after it on the line) up to the next "## ".
$changelog = Join-Path $PubDir 'CHANGELOG.md'
if (-not (Test-Path $changelog)) { Stop-Release "no $changelog" }
$head = '^##\s+\[?v?' + [regex]::Escape($Version) + '\]?(\s|$)'
$notes = @(); $inSection = $false
foreach ($l in Get-Content $changelog) {
    if ($l -match '^##\s') {
        if ($inSection) { break }
        if ($l -match $head) { $inSection = $true; continue }
    }
    if ($inSection) { $notes += $l }
}
$notesText = ($notes -join "`n").Trim()
if (-not $inSection -or -not $notesText) { Stop-Release "CHANGELOG.md in $PubDir has no section (or an empty one) for $Version" }
"  changelog section: $(($notes | Where-Object { $_.Trim() }).Count) line(s)"

$extra = @()
if ($ExtraAssets) {
    if (-not (Test-Path $ExtraAssets -PathType Container)) { Stop-Release "-ExtraAssets $ExtraAssets is not a folder" }
    $extra = @(Get-ChildItem $ExtraAssets -File)
    if (-not $extra) { Stop-Release "-ExtraAssets $ExtraAssets is empty" }
    $clash = $extra | Where-Object { $_.Name -in $ZipName, 'SHA256SUMS.txt', 'latest.json' }
    if ($clash) { Stop-Release "extra asset name clashes with a built one: $($clash.Name -join ', ')" }
}
if (-not (Test-Path (Join-Path $Tesseract 'tesseract.exe'))) { Stop-Release "no Tesseract at $Tesseract (-Tesseract)" }

# ---------------------------------------------------------------- 2. test
if ($SkipTests) { Step 'Tests skipped (-SkipTests)' }
else {
    Step "Tests (cargo test --workspace, -j $TestJobs)"
    cargo test --workspace --locked -j $TestJobs
    if ($LASTEXITCODE) { Stop-Release 'tests failed' }
}

# ---------------------------------------------------------------- 3. build
Step 'Build (release, static CRT, local paths remapped)'
$target = $meta.target_directory
# Local paths must not reach the binary (panic locations, file!() in dependencies, OUT_DIR
# includes). Later prefixes win in rustc, so the general ones come first.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$maps = [ordered]@{}
$maps[$env:USERPROFILE] = '/home'
$maps[(Split-Path $repo)] = '/projects'
$maps[$rustupHome] = '/rustup'
$maps[$cargoHome] = '/cargo'
$maps[$target] = '/target'
if ($meta.build_directory) { $maps[$meta.build_directory] = '/build' }
$maps[$repo] = '/src'
$flags = @('-C', 'target-feature=+crt-static')
foreach ($from in $maps.Keys) {
    $forms = @($from, $from.Replace('/', '\'), $from.Replace('\', '/')) | Select-Object -Unique
    foreach ($f in $forms) { $flags += "--remap-path-prefix=$f=$($maps[$from])" }
}
$savedEnc = $env:CARGO_ENCODED_RUSTFLAGS; $savedFlags = $env:RUSTFLAGS
try {
    # CARGO_ENCODED_RUSTFLAGS (unit-separator joined) keeps paths with spaces whole.
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
    Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    $env:VF_COMMIT = $short
    # One program per cargo run: two fat-LTO links at once ran this 16 GB PC out of memory
    # (rustc-LLVM "out of memory", PA-100).
    foreach ($pkg in 'vf-app', 'vf-move') {
        cargo build --release --locked -j $BuildJobs -p $pkg
        if ($LASTEXITCODE) { Stop-Release "build of $pkg failed" }
    }
    # The 32-bit scanning helper: needs `rustup target add i686-pc-windows-msvc`.
    cargo build --release --locked -j $BuildJobs -p vf-scan32 --target i686-pc-windows-msvc
    if ($LASTEXITCODE) { Stop-Release '32-bit helper build failed' }
}
finally {
    if ($null -ne $savedEnc) { $env:CARGO_ENCODED_RUSTFLAGS = $savedEnc } else { Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue }
    if ($null -ne $savedFlags) { $env:RUSTFLAGS = $savedFlags }
    Remove-Item Env:VF_COMMIT -ErrorAction SilentlyContinue
}
$exe = Join-Path $target 'release\vibe-files.exe'
$moveExe = Join-Path $target 'release\vibe-move.exe'
$scanExe = Join-Path $target 'i686-pc-windows-msvc\release\vibe-scan32.exe'
foreach ($e in $exe, $moveExe, $scanExe) { if (-not (Test-Path $e)) { Stop-Release "no build at $e" } }

# Symbols stay on this PC (never shipped), per version, for crash diagnosis.
$pdbs = @(Get-ChildItem (Join-Path $target 'release'), (Join-Path $target 'i686-pc-windows-msvc\release') -Filter '*.pdb' -File -ErrorAction SilentlyContinue |
    Where-Object { $_.BaseName -replace '-', '_' -in 'vibe_files', 'vibe_move', 'vibe_scan32' })
if ($pdbs) {
    $sym = Join-Path $repo "dist\symbols\$Version"
    New-Item -ItemType Directory -Force $sym | Out-Null
    $pdbs | Copy-Item -Destination $sym
    "  symbols archived: $sym"
} else { '  no .pdb files made (debug = false, strip = true)' }

# ---------------------------------------------------------------- 4. package
Step 'Package (portable zip)'
$out = Join-Path $target "dist\$tag"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
$stage = Join-Path $out 'stage'
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe, $moveExe, $scanExe $stage
Copy-Item crates\filer-pdf\assets\pdfium.dll, crates\filer-pdf\assets\pdfium-LICENSE $stage
if (Test-Path crates\filer-pdf\assets\pdfium-licenses) {
    Copy-Item -Recurse crates\filer-pdf\assets\pdfium-licenses (Join-Path $stage 'pdfium-licenses')
}
Copy-Item crates\vf-ingest\assets\TWAINDSM.dll, crates\vf-ingest\assets\TWAINDSM-LICENSE.txt $stage
# Installed as .txt so Notepad opens them (as ClientLedger Trial Balance does).
Copy-Item LICENSE.md (Join-Path $stage 'LICENSE.txt')
Copy-Item NOTICE.md (Join-Path $stage 'NOTICE.txt')
pwsh -NoProfile -File (Join-Path $PSScriptRoot 'third-party-notices.ps1') -Out (Join-Path $stage 'THIRD-PARTY-NOTICES.txt')
if ($LASTEXITCODE) { Stop-Release 'could not write THIRD-PARTY-NOTICES.txt' }
Copy-Item docs\OPERATIONS.md $stage

# Tesseract (Q-25, Q-32): the program, only the DLLs it loads, English + script detection data.
$t = Join-Path $stage 'tesseract'
New-Item -ItemType Directory -Force (Join-Path $t 'tessdata') | Out-Null
Copy-Item (Join-Path $Tesseract 'tesseract.exe') $t
$dlls = python scripts\dll_closure.py $Tesseract tesseract.exe
if ($LASTEXITCODE) { Stop-Release 'could not read the Tesseract DLL list' }
foreach ($d in $dlls) { Copy-Item (Join-Path $Tesseract $d) $t }
foreach ($f in 'eng.traineddata', 'osd.traineddata') {
    Copy-Item (Join-Path $Tesseract "tessdata\$f") (Join-Path $t 'tessdata')
}
Copy-Item (Join-Path $Tesseract 'doc\LICENSE') (Join-Path $t 'LICENSE')
& (Join-Path $t 'tesseract.exe') --version | Select-Object -First 1
# The shipped Tesseract must give its word table the way the program asks for it (0.1.36 to
# 0.1.43 asked through tessdata\configs\tsv, which is not shipped: OCR found no text).
Add-Type -AssemblyName System.Drawing
$probe = Join-Path $env:TEMP ("vf-release-ocr-probe-{0}.png" -f [guid]::NewGuid().ToString('N'))
$probeErr = "$probe.err"
try {
    $bmp = New-Object System.Drawing.Bitmap 1200, 300
    $font = New-Object System.Drawing.Font 'Arial', 40
    $g = [System.Drawing.Graphics]::FromImage($bmp); $g.Clear([System.Drawing.Color]::White)
    $g.DrawString('Release probe 48,250.00', $font, [System.Drawing.Brushes]::Black, 40, 100)
    $g.Dispose(); $font.Dispose(); $bmp.Save($probe); $bmp.Dispose()
    $tsv = & (Join-Path $t 'tesseract.exe') $probe stdout -l eng --psm 3 --dpi 300 --tessdata-dir (Join-Path $t 'tessdata') -c tessedit_create_tsv=1 2>$probeErr
    $said = if (Test-Path $probeErr) { (Get-Content $probeErr -Raw) } else { '' }
    if ($LASTEXITCODE) { Stop-Release "the staged Tesseract failed (exit $LASTEXITCODE): $said" }
    if (-not ($tsv | Where-Object { $_ -match '^5\t' })) { Stop-Release "the staged Tesseract gives no word table: OCR would find no text. $said" }
} finally {
    Remove-Item $probe, $probeErr -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------- 5. sign (only when configured)
# VF_SIGN_CERT_THUMBPRINT: a code-signing certificate in the Windows certificate store.
# VF_SIGN_TIMESTAMP_URL: optional, default DigiCert's RFC 3161 server.
Step 'Sign'
$signed = $false
if ($env:VF_SIGN_CERT_THUMBPRINT) {
    $signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
    if (-not $signtool) {
        $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $signtool) { Stop-Release 'VF_SIGN_CERT_THUMBPRINT is set but signtool.exe was not found (Windows SDK)' }
    $ts = if ($env:VF_SIGN_TIMESTAMP_URL) { $env:VF_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    $toSign = Get-ChildItem $stage -Filter '*.exe' -File | Select-Object -ExpandProperty FullName
    & $signtool sign /sha1 $env:VF_SIGN_CERT_THUMBPRINT /fd sha256 /tr $ts /td sha256 $toSign
    if ($LASTEXITCODE) { Stop-Release 'signing failed' }
    & $signtool verify /pa $toSign
    if ($LASTEXITCODE) { Stop-Release 'signature verification failed' }
    $signed = $true
    '  signed: ' + (($toSign | ForEach-Object { Split-Path $_ -Leaf }) -join ', ')
} else {
    Write-Host '  UNSIGNED: no VF_SIGN_CERT_THUMBPRINT; Windows SmartScreen will warn on first run.' -ForegroundColor Yellow
}

$zip = Join-Path $out $ZipName
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip

# ---------------------------------------------------------------- 6. check
Step 'Check artifacts (tools/check-binary.ps1)'
pwsh -NoProfile -File (Join-Path $PSScriptRoot 'check-binary.ps1') $stage
if ($LASTEXITCODE) { Stop-Release 'check-binary found something that must not ship (above)' }
pwsh -NoProfile -File (Join-Path $PSScriptRoot 'check-binary.ps1') $zip
if ($LASTEXITCODE) { Stop-Release 'check-binary found something that must not ship in the zip (above)' }
if ($extra) {
    foreach ($e in $extra) {
        pwsh -NoProfile -File (Join-Path $PSScriptRoot 'check-binary.ps1') $e.FullName
        if ($LASTEXITCODE) { Stop-Release "check-binary found something that must not ship in $($e.Name) (above)" }
    }
}

# ---------------------------------------------------------------- 7. assets
Step 'Assets'
foreach ($e in $extra) { Copy-Item $e.FullName $out }
$assetFiles = @(Get-Item $zip) + @($extra | ForEach-Object { Get-Item (Join-Path $out $_.Name) })
function Hash([string]$p) { (Get-FileHash $p -Algorithm SHA256).Hash.ToLower() }
$sums = @($assetFiles | ForEach-Object { "$(Hash $_.FullName)  $($_.Name)" })
# The exes inside the zip too, so an unzipped copy can be checked.
foreach ($e in 'vibe-files.exe', 'vibe-move.exe', 'vibe-scan32.exe') { $sums += "$(Hash (Join-Path $stage $e))  $e" }
$sumsPath = Join-Path $out 'SHA256SUMS.txt'
$sums | Set-Content -Encoding ascii $sumsPath
$assetFiles += Get-Item $sumsPath
$notesUrl = "https://github.com/$PubRepo/releases/tag/$tag"
$latest = [ordered]@{
    version     = $Version
    released_at = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    notes_url   = $notesUrl
    assets      = @($assetFiles | ForEach-Object { [ordered]@{ name = $_.Name; sha256 = (Hash $_.FullName); size = $_.Length } })
}
$latestPath = Join-Path $out 'latest.json'
$latest | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM $latestPath
$assetFiles += Get-Item $latestPath
$notesPath = Join-Path $out 'notes.md'
$notesText | Set-Content -Encoding utf8NoBOM $notesPath

# winget manifests (portable zip; the data folder then lives in %LOCALAPPDATA%\Vibe Files).
# InstallerUrl is the per-tag link: winget pins a hash, so it must never follow "latest".
$zipHash = Hash $zip
$w = Join-Path $out "winget\manifests\k\Kisaes\VibeFiles\$Version"
New-Item -ItemType Directory -Force $w | Out-Null
@"
PackageIdentifier: Kisaes.VibeFiles
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $w 'Kisaes.VibeFiles.yaml')
@"
PackageIdentifier: Kisaes.VibeFiles
PackageVersion: $Version
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
    InstallerUrl: https://github.com/$PubRepo/releases/download/$tag/$ZipName
    InstallerSha256: $($zipHash.ToUpper())
ManifestType: installer
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $w 'Kisaes.VibeFiles.installer.yaml')
@"
PackageIdentifier: Kisaes.VibeFiles
PackageVersion: $Version
PackageLocale: en-US
Publisher: Kisaes LLC
PublisherUrl: https://github.com/$PubRepo
PublisherSupportUrl: https://github.com/$PubRepo/issues
PackageName: $App
PackageUrl: https://github.com/$PubRepo
License: VibeMyFirm Community License 1.0
LicenseUrl: https://github.com/$PubRepo/blob/main/LICENSE.md
ShortDescription: Document management for small CPA and tax firms over a plain client folder tree.
ReleaseNotesUrl: $notesUrl
Tags:
  - document-management
  - accounting
  - tax
ManifestType: defaultLocale
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $w 'Kisaes.VibeFiles.locale.en-US.yaml')

foreach ($a in $assetFiles) { "  {0,-30} {1,12:N0} bytes  {2}" -f $a.Name, $a.Length, (Hash $a.FullName) }

# The exe must come from this commit: changes left after the build make it a trial.
$after = git status --porcelain
$trial = [bool]($dirty -or $after)
if ($trial) {
    "Trial build from $commit with uncommitted changes; do not publish." | Set-Content -Encoding ascii (Join-Path $out 'TRIAL.txt')
    if (-not $DryRun) { Stop-Release 'the tree changed during the build; nothing was published' }
}

# ---------------------------------------------------------------- 8. publish
$assetArgs = $assetFiles | ForEach-Object { $_.FullName }
$publish = @(
    @('git', '-C', $PubDir, 'push', 'origin', 'HEAD'),
    (@('gh', 'release', 'create', $tag, '--repo', $PubRepo, '--title', "$App $Version", '--notes-file', $notesPath, '--latest') + $assetArgs),
    @('git', 'tag', '-a', $tag, '-m', "$App $Version", $commit),
    @('git', 'push', 'origin', "refs/tags/$tag")
)
function Show([string[]]$c) { ($c | ForEach-Object { if ($_ -match '\s') { "'$_'" } else { $_ } }) -join ' ' }
if ($DryRun) {
    Step 'Publish (dry run: not run)'
    foreach ($c in $publish) { "  $(Show $c)" }
    if ($warned) {
        Write-Host "`n  A real run would have stopped before building:" -ForegroundColor Yellow
        $warned | ForEach-Object { Write-Host "   - $_" -ForegroundColor Yellow }
    }
} else {
    Step 'Publish'
    foreach ($c in $publish) {
        "  $(Show $c)"
        & $c[0] $c[1..($c.Count - 1)]
        if ($LASTEXITCODE) { Stop-Release "failed: $(Show $c)" }
    }
}

# ---------------------------------------------------------------- 9. report
Step 'Done'
"  $App $Version from $short$(if ($signed) { ', signed' } else { ', unsigned' })$(if ($trial) { ', TRIAL (uncommitted changes)' })$(if ($DryRun) { ', DRY RUN: nothing published' })"
"  Download (this version): https://github.com/$PubRepo/releases/download/$tag/$ZipName"
"  Download (always latest): https://github.com/$PubRepo/releases/latest/download/$ZipName"
"  Update metadata:          https://github.com/$PubRepo/releases/latest/download/latest.json"
"  Release page:             $notesUrl"
foreach ($a in $assetFiles) { "  SHA-256 $(Hash $a.FullName)  $($a.Name)" }
"  Files: $out"
"  winget manifests: $w"
