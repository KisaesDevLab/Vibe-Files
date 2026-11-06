# Speaks each line of a narration JSON file ([{name, say}]) into <OutDir>\<name>.wav with a
# Windows (SAPI) voice. Windows PowerShell 5.1 only: System.Speech is a .NET Framework
# assembly. Called by ..\walkthrough-video.ps1.
param(
    [Parameter(Mandatory)][string]$Json,
    [Parameter(Mandatory)][string]$OutDir,
    [string]$Voice = 'Microsoft David Desktop',
    [int]$Rate = 0
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$lines = Get-Content -Raw -Encoding UTF8 $Json | ConvertFrom-Json
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(48000,
    [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
    [System.Speech.AudioFormat.AudioChannel]::Mono)
foreach ($l in $lines) {
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
    try { $s.SelectVoice($Voice) } catch { Write-Warning "voice '$Voice' not installed; using the default" }
    $s.Rate = $Rate
    $s.SetOutputToWaveFile((Join-Path $OutDir "$($l.name).wav"), $fmt)
    $s.Speak($l.say)
    $s.Dispose()
}
