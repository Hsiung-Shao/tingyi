# 自動化 smoke test 用:等 llama-server /health 就緒(或 -NoWait),再用 Windows TTS 透過預設喇叭唸句子,
# 讓 WASAPI loopback 擷取。搭配 `tingyi-bench live` 或 Tauri app 使用。
# 用法:powershell -ExecutionPolicy Bypass -File scripts\tts-smoke.ps1 -Lines docs\bench\tts-en.txt [-Port 8178] [-ExtraDelaySec 6] [-NoWait]
param(
    [string]$Lines = "docs\bench\tts-en.txt",
    [int]$Port = 8178,
    [int]$ExtraDelaySec = 6,
    [int]$MaxWaitSec = 120,
    [string]$Voice = "Microsoft Zira Desktop",
    [int]$GapMs = 1800,
    [switch]$NoWait
)
$ErrorActionPreference = 'Stop'

if (-not $NoWait) {
    $deadline = (Get-Date).AddSeconds($MaxWaitSec)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/health" -UseBasicParsing -TimeoutSec 2
            if ($r.StatusCode -eq 200) { $ready = $true; break }
        } catch {
            # llama-server 載入模型期間連線被拒或回 503 都屬預期,忽略後繼續輪詢
        }
        Start-Sleep -Milliseconds 500
    }
    if (-not $ready) { Write-Host "llama-server 在 $MaxWaitSec 秒內未就緒,仍繼續唸"; }
    else { Write-Host "llama-server 就緒,等 $ExtraDelaySec 秒讓管線載完模型" }
    Start-Sleep -Seconds $ExtraDelaySec
}

Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
try { $s.SelectVoice($Voice) } catch { Write-Host "找不到語音 $Voice,用預設" }
$s.Rate = 0
$s.Volume = 100
$texts = Get-Content -Encoding UTF8 $Lines | Where-Object { $_.Trim() -and -not $_.StartsWith('#') }
$i = 0
foreach ($t in $texts) {
    $i++
    Write-Host ("[{0}] {1:HH:mm:ss.fff} {2}" -f $i, (Get-Date), $t)
    $s.Speak($t)
    Start-Sleep -Milliseconds $GapMs
}
Write-Host "TTS_DONE $i 句"
