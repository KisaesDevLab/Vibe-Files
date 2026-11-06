# Builds the landing-page walkthrough video (1920x1080 MP4 with a synthetic voice and burned-in
# captions, plus SRT and VTT caption files) from frames of the real app.
#
#   pwsh -File scripts\walkthrough-video.ps1 -Ffmpeg C:\path\to\ffmpeg.exe
#
# 1. Frames: `cargo test -p vf-app --bin vibe-files walkthrough -- --ignored` renders the app on
#    the generated fixture cabinet (invented clients only). This PC's name, the Windows login and
#    the temp path show in some screens, so the test runs with neutral ones (OFFICE-PC1\sam,
#    C:\VibeDemo) and never with the real ones.
# 2. Voice: one clip per scene; each scene lasts as long as its narration. Default: a Microsoft
#    Edge neural voice through scripts\walkthrough\edge_tts_lines.py (-Python must be a Python
#    with the `edge-tts` package; the narration text goes to Microsoft's read-aloud service).
#    -Engine sapi uses the voices built into Windows (scripts\walkthrough\tts.ps1) offline.
# 3. Video: ffmpeg, one segment per scene, joined.
#
# Output: target\walkthrough\vibe-files-walkthrough.mp4 (+ .srt, .vtt). Not committed.
param(
    [Parameter(Mandatory)][string]$Ffmpeg,
    [string]$Out = '',
    [ValidateSet('edge', 'sapi')][string]$Engine = 'edge',
    [string]$Voice = '',
    [string]$Python = 'python',
    [switch]$SkipFrames
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (-not $Out) { $Out = Join-Path $repo 'target\walkthrough' }
$frames = Join-Path $Out 'frames'
$work = Join-Path $Out 'work'
New-Item -ItemType Directory -Force $frames, $work | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)

# Scenes: frames (from the test, or 'card' over a blurred frame), what is said, the caption.
$scenes = @(
    @{ name = '00-title'; frames = @('10-cabinet-dark'); card = @('Vibe Files', 'Document management for small CPA and tax firms')
       say = 'Meet Vibe Files: document management for small CPA and tax firms.'; caption = '' },
    @{ name = '01'; frames = @('01-cabinet')
       say = 'Vibe Files works on the client folders you already have. Every document stays a plain file, in a plain Windows folder.'
       caption = 'Works on the client folders you already have. Every document stays a plain file, in a plain Windows folder.' },
    @{ name = '02'; frames = @('02-client-preview')
       say = 'Open a client, pick a year, and preview any document right beside the list.'
       caption = 'Open a client, pick a year, and preview any document beside the list.' },
    @{ name = '03'; frames = @('03-details')
       say = 'Each document carries a doc type, a tax year and a status. And every change can be undone.'
       caption = 'Each document carries a doc type, a tax year and a status. Every change can be undone.' },
    @{ name = '04'; frames = @('04-intake')
       say = 'New scans land in Intake. It reads each file name, suggests the client, year and folder, flags anything that would collide, and files the batch in one step.'
       caption = 'Intake reads each file name, suggests the client, year and folder, flags collisions, and files the batch in one step.' },
    @{ name = '05'; frames = @('05-search-0', '05-search-1', '05-search-2')
       say = 'Search finds any document by name, client, year, or the words inside it, scanned pages included.'
       caption = 'Search by name, client, year, or the words inside a document, scanned pages included.' },
    @{ name = '06'; frames = @('06-viewer')
       say = 'The built-in viewer lets you annotate, stamp, redact, and tick-mark PDFs.'
       caption = 'The built-in viewer: annotate, stamp, redact and tick-mark PDFs.' },
    @{ name = '07'; frames = @('07-sync')
       say = 'Optional sync to your own Backblaze B2 bucket keeps an off-site copy, encrypted on your PC before it leaves.'
       caption = 'Optional sync to your own Backblaze B2 bucket, encrypted on your PC before it leaves.' },
    @{ name = '08'; frames = @('08-servers')
       say = 'Run it on one PC, across the office network, or on several Remote Desktop servers sharing one cabinet.'
       caption = 'One PC, the office network, or several Remote Desktop servers sharing one cabinet.' },
    @{ name = '09'; frames = @('09-retention')
       say = 'Retention classes, legal holds, and a tamper-evident audit trail keep your record keeping in order.'
       caption = 'Retention classes, legal holds and a tamper-evident audit trail.' },
    @{ name = '10'; frames = @('10-cabinet-dark')
       say = "Light or dark, Vibe Files keeps every client's documents in order, and your files always open without it."
       caption = "Light or dark. And your files always open without Vibe Files." },
    @{ name = '99-end'; frames = @('10-cabinet-dark'); card = @('Vibe Files', 'Plain files, organized.')
       say = 'Vibe Files. Plain files, organized.'; caption = '' }
)

# 1. Frames, under a neutral login, PC name and temp folder.
if (-not $SkipFrames) {
    $demo = Join-Path $env:SystemDrive 'VibeDemo'
    New-Item -ItemType Directory -Force $demo | Out-Null
    $keep = @{}
    foreach ($k in 'USERNAME', 'USERDOMAIN', 'COMPUTERNAME', 'TEMP', 'TMP', 'VF_WALKTHROUGH_DIR') {
        $keep[$k] = [Environment]::GetEnvironmentVariable($k)
    }
    try {
        $env:USERNAME = 'sam'; $env:USERDOMAIN = 'OFFICE-PC1'; $env:COMPUTERNAME = 'OFFICE-PC1'
        $env:TEMP = $demo; $env:TMP = $demo; $env:VF_WALKTHROUGH_DIR = $frames
        Push-Location $repo
        cargo test -p vf-app --bin vibe-files walkthrough -- --ignored
        if ($LASTEXITCODE -ne 0) { throw 'rendering the frames failed' }
    } finally {
        Pop-Location
        foreach ($k in $keep.Keys) { [Environment]::SetEnvironmentVariable($k, $keep[$k]) }
    }
}

# 2. Voice.
$json = Join-Path $work 'narration.json'
[IO.File]::WriteAllText($json, (ConvertTo-Json @($scenes | ForEach-Object { @{ name = $_.name; say = $_.say } })), $utf8)
if ($Engine -eq 'edge') {
    if (-not $Voice) { $Voice = 'en-US-AndrewNeural' }
    & $Python (Join-Path $PSScriptRoot 'walkthrough\edge_tts_lines.py') $json $work $Voice
    if ($LASTEXITCODE -ne 0) { throw 'speech failed (is edge-tts installed in that Python?)' }
    foreach ($s in $scenes) {
        & $Ffmpeg -hide_banner -loglevel error -y -i (Join-Path $work "$($s.name).mp3") -ar 48000 -ac 1 -c:a pcm_s16le (Join-Path $work "$($s.name).wav")
        if ($LASTEXITCODE -ne 0) { throw "converting $($s.name) failed" }
    }
} else {
    if (-not $Voice) { $Voice = 'Microsoft David Desktop' }
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'walkthrough\tts.ps1') -Json $json -OutDir $work -Voice $Voice
    if ($LASTEXITCODE -ne 0) { throw 'speech failed' }
}

function WavSeconds([string]$path) {
    $b = [IO.File]::ReadAllBytes($path)
    $rate = [BitConverter]::ToInt32($b, 28)
    $i = 12
    while ($i -lt $b.Length - 8) {
        $id = [Text.Encoding]::ASCII.GetString($b, $i, 4)
        $size = [BitConverter]::ToInt32($b, $i + 4)
        if ($id -eq 'data') { return $size / $rate }
        $i += 8 + $size
    }
    throw "no audio in $path"
}

function Wrap([string]$text, [int]$width = 56) {
    $lines = @(); $line = ''
    foreach ($w in $text -split ' ') {
        if ($line.Length -and ($line.Length + 1 + $w.Length) -gt $width) { $lines += $line; $line = $w }
        elseif ($line.Length) { $line += " $w" } else { $line = $w }
    }
    if ($line) { $lines += $line }
    $lines -join "`n"
}

# 3. Video: one segment per scene.
$font = 'C:\Windows\Fonts\segoeuisb.ttf'
if (-not (Test-Path $font)) { $font = 'C:\Windows\Fonts\segoeui.ttf' }
Copy-Item $font (Join-Path $work 'font.ttf') -Force
$fontBold = 'C:\Windows\Fonts\segoeuib.ttf'
if (-not (Test-Path $fontBold)) { $fontBold = $font }
Copy-Item $fontBold (Join-Path $work 'bold.ttf') -Force
$tail = 0.8      # after the narration
$step = 1.0      # each earlier frame of a several-frame scene
$fade = 0.25
$list = @(); $cues = @(); $t = 0.0
Push-Location $work
try {
    foreach ($s in $scenes) {
        $speech = WavSeconds (Join-Path $work "$($s.name).wav")
        $n = $s.frames.Count
        $dur = [Math]::Round([Math]::Max($speech + $tail + 0.3, $step * ($n - 1) + 1.5), 2)
        $ffa = @('-hide_banner', '-loglevel', 'error', '-y')
        for ($i = 0; $i -lt $n; $i++) {
            $d = if ($i -lt $n - 1) { $step } else { $dur - $step * ($n - 1) }
            $ffa += @('-loop', '1', '-framerate', '30', '-t', "$d", '-i', (Join-Path $frames "$($s.frames[$i]).png"))
        }
        $ffa += @('-i', "$($s.name).wav")
        $v = (0..($n - 1) | ForEach-Object { "[$($_):v]" }) -join ''
        $f = "${v}concat=n=${n}:v=1:a=0,scale=1920:1080,setsar=1"
        if ($s.card) {
            [IO.File]::WriteAllText((Join-Path $work "$($s.name)-t1.txt"), $s.card[0], $utf8)
            [IO.File]::WriteAllText((Join-Path $work "$($s.name)-t2.txt"), $s.card[1], $utf8)
            $f += ",boxblur=18:2,eq=brightness=-0.18:saturation=0.5"
            $f += ",drawtext=fontfile=bold.ttf:textfile=$($s.name)-t1.txt:fontsize=128:fontcolor=white:x=(w-text_w)/2:y=(h/2)-150"
            $f += ",drawtext=fontfile=font.ttf:textfile=$($s.name)-t2.txt:fontsize=50:fontcolor=white@0.92:x=(w-text_w)/2:y=(h/2)+20"
        }
        if ($s.caption) {
            [IO.File]::WriteAllText((Join-Path $work "$($s.name)-c.txt"), (Wrap $s.caption), $utf8)
            $f += ",drawtext=fontfile=font.ttf:textfile=$($s.name)-c.txt:fontsize=40:line_spacing=12:fontcolor=white:box=1:boxcolor=0x101828@0.80:boxborderw=24:x=(w-text_w)/2:y=h-text_h-72"
            $cues += [pscustomobject]@{ start = $t + 0.1; end = $t + $dur - 0.1; text = (Wrap $s.caption 60) }
        }
        $f += ",fade=t=in:st=0:d=${fade},fade=t=out:st=$($dur - $fade):d=${fade},format=yuv420p[v];"
        $f += "[$($n):a]aresample=48000,aformat=channel_layouts=stereo,adelay=300|300,apad=whole_dur=${dur}[a]"
        $seg = "$($s.name).mp4"
        $ffa += @('-filter_complex', $f, '-map', '[v]', '-map', '[a]', '-t', "$dur",
            '-c:v', 'libx264', '-preset', 'slow', '-crf', '20', '-r', '30',
            '-c:a', 'aac', '-b:a', '160k', $seg)
        & $Ffmpeg @ffa
        if ($LASTEXITCODE -ne 0) { throw "segment $($s.name) failed" }
        $list += "file '$seg'"
        $t += $dur
    }
    [IO.File]::WriteAllText((Join-Path $work 'list.txt'), ($list -join "`n"), $utf8)
    $mp4 = Join-Path $Out 'vibe-files-walkthrough.mp4'
    & $Ffmpeg -hide_banner -loglevel error -y -f concat -safe 0 -i list.txt -c copy -movflags +faststart $mp4
    if ($LASTEXITCODE -ne 0) { throw 'joining failed' }
} finally {
    Pop-Location
}

# Captions for the web page's <track> (SRT and WebVTT).
function Stamp([double]$s, [string]$sep) {
    $ts = [TimeSpan]::FromSeconds($s)
    '{0:00}:{1:00}:{2:00}{3}{4:000}' -f [int]$ts.TotalHours, $ts.Minutes, $ts.Seconds, $sep, $ts.Milliseconds
}
$srt = @(); $vtt = @('WEBVTT', '')
for ($i = 0; $i -lt $cues.Count; $i++) {
    $c = $cues[$i]
    $srt += @("$($i + 1)", "$(Stamp $c.start ',') --> $(Stamp $c.end ',')", $c.text, '')
    $vtt += @("$(Stamp $c.start '.') --> $(Stamp $c.end '.')", $c.text, '')
}
[IO.File]::WriteAllText((Join-Path $Out 'vibe-files-walkthrough.srt'), ($srt -join "`r`n"), $utf8)
[IO.File]::WriteAllText((Join-Path $Out 'vibe-files-walkthrough.vtt'), ($vtt -join "`n"), $utf8)
'{0} ({1:N1} s)' -f (Join-Path $Out 'vibe-files-walkthrough.mp4'), $t
