# 聽譯(tingyi)模型與執行檔下載腳本。
# 下載到 %LOCALAPPDATA%\tingyi\{models,bin},專案 repo 不含大檔。
# 用法:
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1            # 必要項目
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 -With7B    # 加 7B
#   ... -WithWhisper  加 Whisper large-v3-turbo q8 模型(執行檔需 scripts\build-whisper.ps1 自行編譯)
#   ... -WithSakura   加 SakuraLLM 日→中特化模型(7B IQ4_XS 與 1.5B F16;CC BY-NC-SA 4.0,禁商用)
#   ... -WithReazonSpeech 加日文專用辨識模型 ReazonSpeech k2 v2(Zipformer,Apache-2.0;引擎「專用模型」用)
#   ... -WithParakeet 加英文專用辨識模型 Parakeet TDT 0.6B v2 int8(CC-BY-4.0;引擎「專用模型」用)
#   ... -WithCandidates 加 Seed-X-PPO-7B、LMT-60 4B/8B、TranslateGemma 4B/12B 的 Q4_K_M(翻譯模型對照用,約 22 GB)
#   ... -ListOnly  只列清單不下載
param(
    [switch]$With7B,
    [switch]$With1_8B_Q4,
    [switch]$WithWhisper,
    [switch]$WithSakura,
    [switch]$WithCandidates,
    [switch]$WithReazonSpeech,
    [switch]$WithParakeet,
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
    @{ Name = '3dspeaker CAM++ 說話者模型'; Size = '28 MB'; Dest = (Join-Path $models '3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx');
       Url = 'https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/main/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx'; Source = 'csukuangfj/speaker-embedding-models (Hugging Face);說話者辨識用' },
    @{ Name = 'Hy-MT2-1.8B-Q8_0.gguf'; Size = '1.91 GB'; Dest = (Join-Path $models 'Hy-MT2-1.8B-Q8_0.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q8_0.gguf'; Source = 'tencent/Hy-MT2-1.8B-GGUF (Hugging Face, Apache-2.0)' }
)
if ($With1_8B_Q4) {
    $items += @{ Name = 'Hy-MT2-1.8B-Q4_K_M.gguf'; Size = '1.13 GB'; Dest = (Join-Path $models 'Hy-MT2-1.8B-Q4_K_M.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q4_K_M.gguf'; Source = 'tencent/Hy-MT2-1.8B-GGUF (Hugging Face)' }
}
if ($WithWhisper) {
    $items += @{ Name = 'ggml-large-v3-turbo-q8_0.bin'; Size = '874 MB'; Dest = (Join-Path $models 'ggml-large-v3-turbo-q8_0.bin');
       Url = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin'; Source = 'ggerganov/whisper.cpp (Hugging Face, MIT);執行檔請另跑 scripts\build-whisper.ps1' }
}
if ($WithSakura) {
    $items += @{ Name = 'Sakura-7B-Qwen2.5-v1.0-IQ4_XS.gguf'; Size = '4.25 GB'; Dest = (Join-Path $models 'Sakura-7B-Qwen2.5-v1.0-IQ4_XS.gguf');
       Url = 'https://huggingface.co/SakuraLLM/Sakura-7B-Qwen2.5-v1.0-GGUF/resolve/main/sakura-7b-qwen2.5-v1.0-iq4xs.gguf'; Source = 'SakuraLLM (Hugging Face, CC BY-NC-SA 4.0);日→中特化' }
    $items += @{ Name = 'Sakura-1.5B-Qwen2.5-v1.0-F16.gguf'; Size = '3.56 GB'; Dest = (Join-Path $models 'Sakura-1.5B-Qwen2.5-v1.0-F16.gguf');
       Url = 'https://huggingface.co/SakuraLLM/Sakura-1.5B-Qwen2.5-v1.0-GGUF/resolve/main/sakura-1.5b-qwen2.5-v1.0-fp16.gguf'; Source = 'SakuraLLM (Hugging Face, CC BY-NC-SA 4.0);日→中特化小模型' }
}
if ($WithCandidates) {
    $cands = @(
        @('Seed-X-PPO-7B-Q4_K_M.gguf', '4.56 GB', 'https://huggingface.co/mradermacher/Seed-X-PPO-7B-GGUF/resolve/main/Seed-X-PPO-7B.Q4_K_M.gguf', 'mradermacher (Hugging Face);ByteDance Seed-X,OpenMDW'),
        @('LMT-60-4B-Q4_K_M.gguf', '2.72 GB', 'https://huggingface.co/mradermacher/LMT-60-4B-GGUF/resolve/main/LMT-60-4B.Q4_K_M.gguf', 'mradermacher (Hugging Face);NiuTrans LMT-60,Apache-2.0'),
        @('LMT-60-8B-Q4_K_M.gguf', '5.03 GB', 'https://huggingface.co/mradermacher/LMT-60-8B-GGUF/resolve/main/LMT-60-8B.Q4_K_M.gguf', 'mradermacher (Hugging Face);NiuTrans LMT-60,Apache-2.0'),
        @('TranslateGemma-4B-it-Q4_K_M.gguf', '2.49 GB', 'https://huggingface.co/mradermacher/translategemma-4b-it-GGUF/resolve/main/translategemma-4b-it.Q4_K_M.gguf', 'mradermacher (Hugging Face);Google TranslateGemma,Gemma 授權'),
        @('TranslateGemma-12B-it-Q4_K_M.gguf', '7.30 GB', 'https://huggingface.co/mradermacher/translategemma-12b-it-GGUF/resolve/main/translategemma-12b-it.Q4_K_M.gguf', 'mradermacher (Hugging Face);Google TranslateGemma,Gemma 授權')
    )
    foreach ($c in $cands) {
        $items += @{ Name = $c[0]; Size = $c[1]; Dest = (Join-Path $models $c[0]); Url = $c[2]; Source = $c[3] }
    }
}
# sherpa-onnx 的 tar.bz2 模型包:下載後解壓到 models\<目錄名>(Windows 10 起內建 tar.exe 支援 bz2)
$archives = @()
if ($WithReazonSpeech) {
    $archives += @{ Name = 'sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01'; Size = '713 MB';
       Source = 'k2-fsa/sherpa-onnx (GitHub Releases);reazon-research/reazonspeech-k2-v2,Apache-2.0' }
}
if ($WithParakeet) {
    $archives += @{ Name = 'sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8'; Size = '482 MB';
       Source = 'k2-fsa/sherpa-onnx (GitHub Releases);NVIDIA parakeet-tdt-0.6b-v2,CC-BY-4.0' }
}
foreach ($a in $archives) {
    $items += @{ Name = "$($a.Name).tar.bz2"; Size = $a.Size; Dest = (Join-Path $models "$($a.Name).tar.bz2");
       Url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$($a.Name).tar.bz2"; Source = $a.Source;
       Extracted = (Join-Path $models $a.Name) }
}
if ($With7B) {
    $items += @{ Name = 'Hy-MT2-7B-Q4_K_M.gguf'; Size = '4.62 GB'; Dest = (Join-Path $models 'Hy-MT2-7B-Q4_K_M.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-7B-GGUF/resolve/main/Hy-MT2-7B-Q4_K_M.gguf'; Source = 'tencent/Hy-MT2-7B-GGUF (Hugging Face)' }
}

Write-Host "將下載以下檔案到 $root :"
foreach ($i in $items) {
    $state = if ((Test-Path $i.Dest) -or ($i.Extracted -and (Test-Path $i.Extracted))) { '已存在,略過' } else { '待下載' }
    Write-Host ("  [{0}] {1,-32} {2,8}  來源:{3}" -f $state, $i.Name, $i.Size, $i.Source)
}
if ($ListOnly) { exit 0 }

foreach ($i in $items) {
    if ((Test-Path $i.Dest) -or ($i.Extracted -and (Test-Path $i.Extracted))) { continue }
    Write-Host "下載 $($i.Name) ..."
    $tmp = "$($i.Dest).part"
    & curl.exe -L --retry 3 --progress-bar -o $tmp $i.Url
    if ($LASTEXITCODE -ne 0) { throw "下載失敗:$($i.Url)" }
    Move-Item -Force $tmp $i.Dest
}

foreach ($i in $items | Where-Object { $_.Extracted }) {
    if (Test-Path $i.Extracted) { continue }
    Write-Host "解壓 $($i.Name) ..."
    & tar.exe -xjf $i.Dest -C $models
    if ($LASTEXITCODE -ne 0) { throw "解壓失敗:$($i.Dest)" }
    Remove-Item -Force $i.Dest
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
