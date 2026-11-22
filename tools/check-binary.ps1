#requires -Version 7
# Scans release artifacts for things that must not ship (PA-100 R2 step 6): this PC's Windows
# user name, C:\Projects paths, the private source repo or any other KisaesDevLab repo except the
# public release repo, and common secret/token prefixes.
#
#   pwsh tools/check-binary.ps1 <file-or-folder-or-zip> [...]
#
# Every file is read as bytes and searched both as 8-bit text and as UTF-16LE text (Windows wide
# strings). A folder is searched recursively; a .zip is unpacked to a temp folder first.
# Exit 0: clean. Exit 1: at least one hit (each printed as file, pattern, matched text).
param(
    [Parameter(Mandatory, Position = 0)]
    [string[]]$Path,
    # The one KisaesDevLab repo a shipped file may name.
    [string]$PublicRepo = 'vibe-files-releases'
)
$ErrorActionPreference = 'Stop'

$pub = [regex]::Escape($PublicRepo)
$checks = [ordered]@{
    'Windows user name' = if ($env:USERNAME) { '(?i)(?<![A-Za-z0-9])' + [regex]::Escape($env:USERNAME) + '(?![A-Za-z0-9])' } else { $null }
    'C:\Projects path'  = '(?i)[A-Z]:[\\/]+Projects[\\/]'
    'user profile path' = '(?i)[A-Z]:[\\/]+Users[\\/]+(?!Public[\\/])[^\\/\x00"<>|]{1,64}[\\/]'
    'private repo'      = "(?i)KisaesDevLab/(?!$pub(?![A-Za-z0-9_.-]))[A-Za-z0-9_.-]+"
    'GitHub token'      = '(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})'
    'AWS access key'    = '(?:AKIA|ASIA)[0-9A-Z]{16}'
    'Slack token'       = 'xox[abprs]-[A-Za-z0-9-]{10,}'
    'API secret key'    = '(?:sk|rk)[-_](?:live|test|proj|ant)[-_][A-Za-z0-9_-]{16,}'
    'private key block' = '-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----'
}

$latin1 = [System.Text.Encoding]::Latin1
$utf16 = [System.Text.Encoding]::Unicode
$temps = @()
$files = @()
foreach ($p in $Path) {
    if (-not (Test-Path $p)) { throw "not found: $p" }
    $item = Get-Item $p
    if ($item.PSIsContainer) {
        $files += Get-ChildItem $item -Recurse -File
    } elseif ($item.Extension -eq '.zip') {
        $t = Join-Path ([IO.Path]::GetTempPath()) ("vf-check-" + [guid]::NewGuid().ToString('N'))
        Expand-Archive $item $t
        $temps += $t
        $files += Get-ChildItem $t -Recurse -File
    } else {
        $files += $item
    }
}

$hits = 0
try {
    foreach ($f in $files) {
        $bytes = [IO.File]::ReadAllBytes($f.FullName)
        # Wide strings start on either byte; both alignments are searched.
        $texts = @($latin1.GetString($bytes), $utf16.GetString($bytes))
        if ($bytes.Length -gt 1) { $texts += $utf16.GetString($bytes, 1, $bytes.Length - 1) }
        $seen = @{}
        foreach ($name in $checks.Keys) {
            $rx = $checks[$name]
            if (-not $rx) { continue }
            foreach ($text in $texts) {
                foreach ($m in [regex]::Matches($text, $rx)) {
                    $key = "$name|$($m.Value)"
                    if ($seen[$key]) { continue }
                    $seen[$key] = $true
                    $hits++
                    $shown = if ($name -match 'token|key|secret') { $m.Value.Substring(0, [Math]::Min(8, $m.Value.Length)) + '…' } else { $m.Value }
                    "HIT  $($f.Name)  [$name]  $shown"
                }
            }
        }
    }
} finally {
    foreach ($t in $temps) { Remove-Item -Recurse -Force $t -ErrorAction SilentlyContinue }
}

if ($hits) {
    "check-binary: $hits hit(s) in $($files.Count) file(s)"
    exit 1
}
"check-binary: clean ($($files.Count) file(s))"
exit 0
