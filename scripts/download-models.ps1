# 聽譯(tingyi)模型與執行檔下載腳本。
# 下載到 %LOCALAPPDATA%\tingyi\{models,bin},專案 repo 不含大檔。
# 用法:
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1            # 必要項目
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 -With7B    # 加 7B
#   ... -WithWhisper  加 Whisper large-v3-turbo q8 模型(執行檔需 scriptsuild-whisper.ps1 自行編譯)
#   ... -ListOnly  只列清單不下載
param(
    [switch]$With7B,
    [switch]$With1_8B_Q4,
    [switch]$WithWhisper,
    [switch]$ListOnly
)

$ErrorActionPreference = 'Stop'
$root = Join-Path $env:LOCALAPPDATA 'tingyi'
$models = Join-Path $root 'models'
$bin = Join-Path $root 'bin'
$svDir = Join-Path $models 'sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17'
New-Item -ItemType Directory -Force $models, $bin, $svDir | Out-Null

$llamaBuild = 'b11405'
$items = @(
    @{ Name = "llama-$llamaBuild-bin-win-vulkan-x64.zip"; Size = '33 MB'; Dest = (Join-Path $bin "llama-$llamaBuild-bin-win-vulkan-x64.zip");
       Url = "https://github.com/ggml-org/llama.cpp/releases/download/$llamaBuild/llama-$llamaBuild-bin-win-vulkan-x64.zip"; Source = 'ggml-org/llama.cpp (GitHub Releases)' },
    @{ Name = 'SenseVoice model.int8.onnx'; Size = '237 MB'; Dest = (Join-Path $svDir 'model.int8.onnx');
       Url = 'https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx'; Source = 'csukuangfj (Hugging Face 鏡像,原版,2025-09-09 版為粵語微調不適用)' },
    @{ Name = 'SenseVoice tokens.txt'; Size = '0.3 MB'; Dest = (Join-Path $svDir 'tokens.txt');
       Url = 'https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/tokens.txt'; Source = 'csukuangfj (Hugging Face)' },
    @{ Name = 'silero_vad.onnx'; Size = '0.6 MB'; Dest = (Join-Path $models 'silero_vad.onnx');
       Url = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx'; Source = 'k2-fsa/sherpa-onnx (GitHub Releases)' },
    @{ Name = 'Hy-MT2-1.8B-Q8_0.gguf'; Size = '1.91 GB'; Dest = (Join-Path $models 'Hy-MT2-1.8B-Q8_0.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q8_0.gguf'; Source = 'tencent/Hy-MT2-1.8B-GGUF (Hugging Face, Apache-2.0)' }
)
if ($With1_8B_Q4) {
    $items += @{ Name = 'Hy-MT2-1.8B-Q4_K_M.gguf'; Size = '1.13 GB'; Dest = (Join-Path $models 'Hy-MT2-1.8B-Q4_K_M.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q4_K_M.gguf'; Source = 'tencent/Hy-MT2-1.8B-GGUF (Hugging Face)' }
}
if ($WithWhisper) {
    $items += @{ Name = 'ggml-large-v3-turbo-q8_0.bin'; Size = '874 MB'; Dest = (Join-Path $models 'ggml-large-v3-turbo-q8_0.bin');
       Url = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin'; Source = 'ggerganov/whisper.cpp (Hugging Face, MIT);執行檔請另跑 scriptsuild-whisper.ps1' }
}
if ($With7B) {
    $items += @{ Name = 'Hy-MT2-7B-Q4_K_M.gguf'; Size = '4.62 GB'; Dest = (Join-Path $models 'Hy-MT2-7B-Q4_K_M.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-7B-GGUF/resolve/main/Hy-MT2-7B-Q4_K_M.gguf'; Source = 'tencent/Hy-MT2-7B-GGUF (Hugging Face)' }
}

Write-Host "將下載以下檔案到 $root :"
foreach ($i in $items) {
    $state = if (Test-Path $i.Dest) { '已存在,略過' } else { '待下載' }
    Write-Host ("  [{0}] {1,-32} {2,8}  來源:{3}" -f $state, $i.Name, $i.Size, $i.Source)
}
if ($ListOnly) { exit 0 }

foreach ($i in $items) {
    if (Test-Path $i.Dest) { continue }
    Write-Host "下載 $($i.Name) ..."
    $tmp = "$($i.Dest).part"
    & curl.exe -L --retry 3 --progress-bar -o $tmp $i.Url
    if ($LASTEXITCODE -ne 0) { throw "下載失敗:$($i.Url)" }
    Move-Item -Force $tmp $i.Dest
}

# 解壓 llama-server 到 bin\llama 與 src-tauri\binaries(sidecar 命名需 target triple 後綴)
$zip = Join-Path $bin "llama-$llamaBuild-bin-win-vulkan-x64.zip"
$llamaDir = Join-Path $bin 'llama'
if (-not (Test-Path (Join-Path $llamaDir 'llama-server.exe'))) {
    Write-Host '解壓 llama.cpp ...'
    Expand-Archive -Force $zip $llamaDir
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$sidecarDir = Join-Path $repoRoot 'src-tauri\binaries'
New-Item -ItemType Directory -Force $sidecarDir | Out-Null
Copy-Item -Force (Join-Path $llamaDir 'llama-server.exe') (Join-Path $sidecarDir 'llama-server-x86_64-pc-windows-msvc.exe')
Get-ChildItem $llamaDir -Filter '*.dll' | Copy-Item -Force -Destination $sidecarDir
Write-Host "完成。sidecar 已複製到 $sidecarDir"
