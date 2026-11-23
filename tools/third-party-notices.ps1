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
        # Workspace members (and vendored Filer crates) are Kisaes code under LICENSE.md.
        if ($p -and $p.source) { $crates["$($p.name) $($p.version)"] = $p }
    }
}

$texts = [ordered]@{}   # hash -> @{ text; crates }

# Standard licence texts for crates that name a licence but publish no file of it (Q-143). MIT,
# BSD-3-Clause and BSL-1.0 are the SPDX texts; Apache-2.0 is taken from a crate that ships it as LICENSE-APACHE.
$standard = [ordered]@{
    'MIT'     = @'
MIT License

Copyright (c) the authors named above for each crate

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute,
sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT
OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
'@
    'BSD-3-Clause' = @'
BSD 3-Clause License

Copyright (c) the authors named above for each crate

Redistribution and use in source and binary forms, with or without modification, are permitted
provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions
   and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice, this list of
   conditions and the following disclaimer in the documentation and/or other materials provided
   with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors may be used to
   endorse or promote products derived from this software without specific prior written
   permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER
IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
'@
    'BSL-1.0' = @'
Boost Software License - Version 1.0 - August 17th, 2003

Permission is hereby granted, free of charge, to any person or organization obtaining a copy of
the software and accompanying documentation covered by this license (the "Software") to use,
reproduce, display, distribute, execute, and transmit the Software, and to prepare derivative
works of the Software, and to permit third-parties to whom the Software is furnished to do so,
all subject to the following:

The copyright notices in the Software and this entire statement, including the above license
grant, this restriction and the following disclaimer, must be included in all copies of the
Software, in whole or in part, and all derivative works of the Software, unless such copies or
derivative works are solely in the form of machine-executable object code generated by a source
language processor.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, TITLE AND
NON-INFRINGEMENT. IN NO EVENT SHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE
LIABLE FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
'@
}
foreach ($k in ($crates.Keys | Sort-Object)) {
    $f = Get-ChildItem (Split-Path $crates[$k].manifest_path) -File -Filter 'LICENSE-APACHE*' | Select-Object -First 1
    if ($f -and (Get-Content $f.FullName -Raw) -match 'Apache License\s+Version 2\.0') { $standard['Apache-2.0'] = (Get-Content $f.FullName -Raw).Trim(); break }
}
$standardUsers = [ordered]@{}   # licence id -> crates that use its standard text

# "(A OR B) AND C AND D" -> "(A OR B)", "C", "D" (AND outside brackets only).
function Split-TopLevelAnd([string]$expr) {
    $parts = @(); $depth = 0; $cur = ''
    foreach ($tok in ($expr -split '(\(|\)|\s+)' | Where-Object { $_ -ne '' })) {
        if ($tok -eq '(') { $depth++ } elseif ($tok -eq ')') { $depth-- }
        if ($depth -eq 0 -and $tok -eq 'AND') { $parts += $cur.Trim(); $cur = ''; continue }
        $cur += if ($tok -match '^\s+$') { ' ' } else { $tok }
    }
    if ($cur.Trim()) { $parts += $cur.Trim() }
    $parts | ForEach-Object { $_ -replace '\(\s+', '(' -replace '\s+\)', ')' }
}
# Whether a text found in a crate's subfolder is the text of licence $id. Only font licences
# count: an MIT text there belongs to a font (its authors' copyright), not to the crate's code.
function Test-Covered([string]$id, [string]$found) {
    $marks = @{
        'OFL-1.1'         = 'SIL OPEN FONT LICEN[CS]E'
        'Ubuntu-font-1.0' = 'UBUNTU FONT LICEN[CS]E'
    }
    $marks.Contains($id) -and $found -match "(?i)$($marks[$id])"
}
$entries = @()
foreach ($k in ($crates.Keys | Sort-Object)) {
    $p = $crates[$k]
    $dir = Split-Path $p.manifest_path
    $files = @(Get-ChildItem $dir -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)' } | Sort-Object Name)
    $fromSubfolder = -not $files
    if (-not $files) {
        # Some crates keep their texts one folder down (epaint_default_fonts: fonts\OFL.txt, UFL.txt,
        # and a .txt beside each font naming its licence).
        $files = @(Get-ChildItem $dir -Directory | Get-ChildItem -File | Where-Object {
                # Real licence file names only (not src\license.rs): LICENSE*, COPYING*, OFL.txt,
                # UFL.txt, or <name>-license.txt.
                ($_.Extension -in '', '.txt', '.md' -and
                 $_.Name -match '(?i)^(licen[cs]e|copying|ofl|ufl)|[-_]licen[cs]e\.(txt|md)$') -or
                ($_.Extension -eq '.txt' -and (Test-Path (Join-Path $_.DirectoryName "$($_.BaseName).ttf")))
            } | Sort-Object FullName)
    }
    $refs = @()
    $found = ''   # the texts found, to tell which licences of the expression they cover
    foreach ($f in $files) {
        $t = (Get-Content $f.FullName -Raw).Trim()
        if (-not $t) { continue }
        $found += "`n$t" 
        $h = (Get-FileHash -InputStream ([IO.MemoryStream]::new([Text.Encoding]::UTF8.GetBytes($t))) -Algorithm SHA256).Hash.Substring(0, 12)
        if (-not $texts.Contains($h)) { $texts[$h] = @{ text = $t; crates = @() } }
        $texts[$h].crates += "$($p.name) $($p.version) ($($f.Name))"
        $refs += "[$h]"
    }
    $lic = if ($p.license) { $p.license } elseif ($p.license_file) { "see $($p.license_file)" } else { 'not stated' }
    if ($refs -and $fromSubfolder) {
        # Its subfolder may hold only the fonts' licences. Every AND term of the expression that
        # no text found covers (the code's own "A OR B", a plain "MIT") gets a standard text.
        $added = @()
        foreach ($term in (Split-TopLevelAnd $lic)) {
            $alts = @($term -replace '^\(|\)$', '' -split '\s+OR\s+' | ForEach-Object Trim)
            if ($alts | Where-Object { Test-Covered $_ $found }) { continue }
            $id = @($alts | Where-Object { $standard.Contains($_) })[0]
            if (-not $id) { throw "$($p.name) $($p.version) ($lic): no text found and no standard text for '$term'" }
            if (-not $standardUsers.Contains($id)) { $standardUsers[$id] = @() }
            $who = if ($p.authors) { $p.authors -join ', ' -replace '\s*<[^>]*>', '' } else { "the $($p.name) authors" }
            $standardUsers[$id] += "$($p.name) $($p.version) (copyright $who)"
            $added += $id
        }
        $extra = if ($added) { "; also under $($added -join ' AND '), standard text at the end" } else { '' }
        $entries += ("{0} {1}: {2}  texts {3}{4}" -f $p.name, $p.version, $lic, ($refs -join ' '), $extra)
    } elseif ($refs) {
        $entries += ("{0} {1}: {2}  texts {3}" -f $p.name, $p.version, $lic, ($refs -join ' '))
    } else {
        # No text of its own: the standard text of the licence named goes at the end (Q-143). For
        # "A OR B" one choice is enough; take the first whose every licence has a standard text.
        $pick = $null
        if ($lic -match '[()]') { throw "$($p.name) $($p.version) ($lic) publishes no licence file and its licence is a nested expression: add its texts by hand" }
        foreach ($alt in ($lic -replace '[()]', '' -split '\s+OR\s+')) {
            $ids = @($alt -split '\s+AND\s+' | ForEach-Object Trim)
            if (-not ($ids | Where-Object { -not $standard.Contains($_) })) { $pick = $ids; break }
        }
        if (-not $pick) { throw "$($p.name) $($p.version) ($lic) publishes no licence file and no standard text covers it: add one to `$standard" }
        foreach ($id in $pick) {
            if (-not $standardUsers.Contains($id)) { $standardUsers[$id] = @() }
            $who = if ($p.authors) { $p.authors -join ', ' -replace '\s*<[^>]*>', '' } else { "the $($p.name) authors" }
            $standardUsers[$id] += "$($p.name) $($p.version) (copyright $who)"
        }
        $entries += ("{0} {1}: {2}  publishes no licence file; used under {3}, standard text at the end" -f $p.name, $p.version, $lic, ($pick -join ' AND '))
    }
}

$sb = [Text.StringBuilder]::new()
$null = $sb.AppendLine('THIRD-PARTY NOTICES FOR VIBE FILES')
$null = $sb.AppendLine('')
$null = $sb.AppendLine('Vibe Files is licensed under the VibeMyFirm Community License 1.0 (LICENSE.txt; NOTICE.txt). It includes')
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
$null = $sb.AppendLine('')
$null = $sb.AppendLine('== Standard licence texts (for the libraries above that publish no licence file)')
foreach ($id in $standardUsers.Keys) {
    $null = $sb.AppendLine('')
    $null = $sb.AppendLine("---- $id, used by: $($standardUsers[$id] -join ', ')")
    $null = $sb.AppendLine('')
    $null = $sb.AppendLine($standard[$id].Trim())
}
[IO.File]::WriteAllText($Out, $sb.ToString().Replace("`r`n", "`n").Replace("`n", "`r`n"))
"third-party notices: $($crates.Count) crates, $($texts.Count) distinct texts -> $Out"
