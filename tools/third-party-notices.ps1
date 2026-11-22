#requires -Version 7
# Writes THIRD-PARTY-NOTICES.txt for the shipped programs (PA-100): every crates.io dependency
# compiled into vibe-files.exe, vibe-move.exe and vibe-scan32.exe, with its licence expression and
# the licence/notice files it publishes (identical texts printed once), plus the bundled DLLs and
# Tesseract. Paths on this PC are never written: only crate names, versions and licence texts.
#
#   pwsh tools/third-party-notices.ps1 -Out <file>
param([Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

$meta = cargo metadata --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE) { throw 'cargo metadata failed' }
$byId = @{}
foreach ($p in $meta.packages) { $byId["$($p.name) $($p.version)"] = $p }

# Normal (linked) dependencies only, for the targets each program is built for.
$lines = @()
$lines += cargo tree -p vf-app -p vf-move -e normal --target x86_64-pc-windows-msvc --prefix none --format '{p}' --locked
if ($LASTEXITCODE) { throw 'cargo tree failed (x64)' }
$lines += cargo tree -p vf-scan32 -e normal --target i686-pc-windows-msvc --prefix none --format '{p}' --locked
if ($LASTEXITCODE) { throw 'cargo tree failed (x86)' }
$crates = @{}
foreach ($l in $lines) {
    if ($l -match '^(\S+) v(\S+)') {
        $p = $byId["$($Matches[1]) $($Matches[2])"]
        # Workspace members (and vendored Filer crates) are Kisaes code under the EULA.
        if ($p -and $p.source) { $crates["$($p.name) $($p.version)"] = $p }
    }
}

$texts = [ordered]@{}   # hash -> @{ text; crates }
$entries = @()
foreach ($k in ($crates.Keys | Sort-Object)) {
    $p = $crates[$k]
    $dir = Split-Path $p.manifest_path
    $files = Get-ChildItem $dir -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)' } | Sort-Object Name
    $refs = @()
    foreach ($f in $files) {
        $t = (Get-Content $f.FullName -Raw).Trim()
        if (-not $t) { continue }
        $h = (Get-FileHash -InputStream ([IO.MemoryStream]::new([Text.Encoding]::UTF8.GetBytes($t))) -Algorithm SHA256).Hash.Substring(0, 12)
        if (-not $texts.Contains($h)) { $texts[$h] = @{ text = $t; crates = @() } }
        $texts[$h].crates += "$($p.name) $($p.version) ($($f.Name))"
        $refs += "[$h]"
    }
    $lic = if ($p.license) { $p.license } elseif ($p.license_file) { "see $($p.license_file)" } else { 'not stated' }
    $entries += ("{0} {1}: {2}{3}" -f $p.name, $p.version, $lic, $(if ($refs) { '  texts ' + ($refs -join ' ') } else { '  (publishes no licence file; the licence named applies)' }))
}

$sb = [Text.StringBuilder]::new()
$null = $sb.AppendLine('THIRD-PARTY NOTICES FOR VIBE FILES')
$null = $sb.AppendLine('')
$null = $sb.AppendLine('Vibe Files is licensed under the Kisaes end user licence agreement (LICENSE). It includes')
$null = $sb.AppendLine('the third-party components below, each under its own licence. Nothing in the Vibe Files')
$null = $sb.AppendLine('licence limits your rights under these licences for these components.')
$null = $sb.AppendLine('')
$null = $sb.AppendLine('== Bundled programs and libraries')
$null = $sb.AppendLine('')
$null = $sb.AppendLine('PDFium (pdfium.dll): BSD-3-Clause and Apache-2.0 with the licences of its parts; see')
$null = $sb.AppendLine('  pdfium-LICENSE and pdfium-licenses\ beside the program.')
$null = $sb.AppendLine('TWAIN Data Source Manager (TWAINDSM.dll): LGPL-2.1-or-later; see TWAINDSM-LICENSE.txt.')
$null = $sb.AppendLine('  It is an unmodified library loaded at run time; you may replace it with another build.')
$null = $sb.AppendLine('  Its source is published by the TWAIN Working Group at https://github.com/twain/twain-dsm.')
$null = $sb.AppendLine('Tesseract OCR (tesseract\): Apache-2.0; see tesseract\LICENSE. Its source is at')
$null = $sb.AppendLine('  https://github.com/tesseract-ocr/tesseract.')
$null = $sb.AppendLine('')
$null = $sb.AppendLine("== Rust libraries compiled into the programs ($($crates.Count))")
$null = $sb.AppendLine('')
foreach ($e in $entries) { $null = $sb.AppendLine($e) }
$null = $sb.AppendLine('')
$null = $sb.AppendLine('== Licence and notice texts')
foreach ($h in $texts.Keys) {
    $null = $sb.AppendLine('')
    $null = $sb.AppendLine("---- [$h] used by: $($texts[$h].crates -join ', ')")
    $null = $sb.AppendLine('')
    $null = $sb.AppendLine($texts[$h].text)
}
[IO.File]::WriteAllText($Out, $sb.ToString().Replace("`r`n", "`n").Replace("`n", "`r`n"))
"third-party notices: $($crates.Count) crates, $($texts.Count) distinct texts -> $Out"
