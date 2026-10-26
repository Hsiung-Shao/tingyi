# Verify helper: speak lines with Windows TTS from a process that owns a visible (minimized, never activated)
# top-level window, so the audio source dropdown lists it as "[powershell.exe]: <Title>".
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File tts-window.ps1 -Lines <file> [-GapMs 1200] [-Title "tingyi verify TTS"]
param(
    [Parameter(Mandatory = $true)][string]$Lines,
    [int]$GapMs = 1200,
    [string]$Title = "tingyi verify TTS",
    [string]$Voice = "Microsoft Zira Desktop"
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Speech
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class TtsWin {
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
}
"@

$form = New-Object System.Windows.Forms.Form
$form.Text = $Title
$form.ShowInTaskbar = $true
$null = $form.Handle            # create the window without showing it
[void][TtsWin]::ShowWindow($form.Handle, 7)   # SW_SHOWMINNOACTIVE: visible, minimized, does not take focus

$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
try { $s.SelectVoice($Voice) } catch { Write-Host "voice $Voice not found, using default" }
$s.Rate = 0
$s.Volume = 100
$texts = Get-Content -Encoding UTF8 $Lines | Where-Object { $_.Trim() -and -not $_.StartsWith('#') }
while ($true) {
    foreach ($t in $texts) {
        $s.Speak($t)
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds $GapMs
        [System.Windows.Forms.Application]::DoEvents()
    }
}
