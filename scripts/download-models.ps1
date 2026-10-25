# 聽譯(tingyi)模型與執行檔下載腳本。
# 下載到資料目錄的 {models,bin}(-DataDir > 環境變數 TINGYI_DATA_DIR > %LOCALAPPDATA%\tingyi),專案 repo 不含大檔。
# 每個檔案都比對寫死的 SHA256,不符就刪掉並中止;Hugging Face 網址固定到 commit,換版本要連 Sha256 一起改。
# 用法:
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1            # 必要項目
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 -With7B    # 加 7B
#   ... -WithWhisper  加 Whisper large-v3-turbo q8 模型(執行檔需 scripts\build-whisper.ps1 自行編譯)
#   ... -WithSakura   加 SakuraLLM 日→中特化模型(7B IQ4_XS 與 1.5B F16;CC BY-NC-SA 4.0,禁商用)
#   ... -WithReazonSpeech 加日文專用辨識模型 ReazonSpeech k2 v2(Zipformer,Apache-2.0;引擎「專用模型」用)
#   ... -WithParakeet 加英文專用辨識模型 Parakeet TDT 0.6B v2 int8(CC-BY-4.0;引擎「專用模型」用)
#   ... -WithCandidates 加 Seed-X-PPO-7B、LMT-60 4B/8B、TranslateGemma 4B/12B 的 Q4_K_M(翻譯模型對照用,約 22 GB)
#   ... -DataDir <目錄>  下載到指定的資料目錄
#   ... -Verify    已存在的檔案也重新校驗 SHA256(含已解壓的專用模型與 llama 執行檔;不重新下載;壓縮檔每次都校驗)
#   ... -ListOnly  只列清單不下載
param(
    [switch]$With7B,
    [switch]$With1_8B_Q4,
    [switch]$WithWhisper,
    [switch]$WithSakura,
    [switch]$WithCandidates,
    [switch]$WithReazonSpeech,
    [switch]$WithParakeet,
    [string]$DataDir = '',
    [switch]$Verify,
    [switch]$ListOnly
)

$ErrorActionPreference = 'Stop'
$root = if ($DataDir) { $DataDir } elseif ($env:TINGYI_DATA_DIR) { $env:TINGYI_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'tingyi' }
$models = Join-Path $root 'models'
$bin = Join-Path $root 'bin'
$svDir = Join-Path $models 'sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17'
New-Item -ItemType Directory -Force $models, $bin, $svDir | Out-Null

$llamaBuild = 'b11405'
$items = @(
    @{ Name = "llama-$llamaBuild-bin-win-vulkan-x64.zip"; Size = '33 MB'; Dest = (Join-Path $bin "llama-$llamaBuild-bin-win-vulkan-x64.zip");
       Url = "https://github.com/ggml-org/llama.cpp/releases/download/$llamaBuild/llama-$llamaBuild-bin-win-vulkan-x64.zip"; Source = 'ggml-org/llama.cpp (GitHub Releases)';
       Sha256 = '0f3764246cc4c365153dc33a24448921526d91bba5ee3195308fa692d2031884' },
    @{ Name = 'SenseVoice model.int8.onnx'; Size = '237 MB'; Dest = (Join-Path $svDir 'model.int8.onnx');
       Url = 'https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx'; Source = 'csukuangfj (Hugging Face 鏡像,原版,2025-09-09 版為粵語微調不適用)';
       Sha256 = 'c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51' },
    @{ Name = 'SenseVoice tokens.txt'; Size = '0.3 MB'; Dest = (Join-Path $svDir 'tokens.txt');
       Url = 'https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt'; Source = 'csukuangfj (Hugging Face)';
       Sha256 = 'f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc' },
    @{ Name = 'silero_vad.onnx'; Size = '0.6 MB'; Dest = (Join-Path $models 'silero_vad.onnx');
       Url = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx'; Source = 'k2-fsa/sherpa-onnx (GitHub Releases)';
       Sha256 = '9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6' },
    @{ Name = '3dspeaker CAM++ 說話者模型'; Size = '28 MB'; Dest = (Join-Path $models '3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx');
       Url = 'https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/0743f301363dec56491a490f6d6cbc9d67f9a3bf/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx'; Source = 'csukuangfj/speaker-embedding-models (Hugging Face);說話者辨識用';
       Sha256 = 'aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2' },
    @{ Name = 'Hy-MT2-1.8B-Q8_0.gguf'; Size = '1.91 GB'; Dest = (Join-Path $models 'Hy-MT2-1.8B-Q8_0.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/a0c709d9fac510f2c807aa3af52872340dc37a4a/Hy-MT2-1.8B-Q8_0.gguf'; Source = 'tencent/Hy-MT2-1.8B-GGUF (Hugging Face, Apache-2.0)';
       Sha256 = '5c3fe0b1408a5ceb0143184ef247b11b579c525f4b02b060e6c851bb76fef1a4' }
)
if ($With1_8B_Q4) {
    $items += @{ Name = 'Hy-MT2-1.8B-Q4_K_M.gguf'; Size = '1.13 GB'; Dest = (Join-Path $models 'Hy-MT2-1.8B-Q4_K_M.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/a0c709d9fac510f2c807aa3af52872340dc37a4a/Hy-MT2-1.8B-Q4_K_M.gguf'; Source = 'tencent/Hy-MT2-1.8B-GGUF (Hugging Face)';
       Sha256 = 'dc5f44fcf1fa496ee7ad725982c0c8c553a4de00259b53af84c4b89fb0c06699' }
}
if ($WithWhisper) {
    $items += @{ Name = 'ggml-large-v3-turbo-q8_0.bin'; Size = '874 MB'; Dest = (Join-Path $models 'ggml-large-v3-turbo-q8_0.bin');
       Url = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q8_0.bin'; Source = 'ggerganov/whisper.cpp (Hugging Face, MIT);執行檔請另跑 scripts\build-whisper.ps1';
       Sha256 = '317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1' }
}
if ($WithSakura) {
    $items += @{ Name = 'Sakura-7B-Qwen2.5-v1.0-IQ4_XS.gguf'; Size = '4.25 GB'; Dest = (Join-Path $models 'Sakura-7B-Qwen2.5-v1.0-IQ4_XS.gguf');
       Url = 'https://huggingface.co/SakuraLLM/Sakura-7B-Qwen2.5-v1.0-GGUF/resolve/24393225dfab4750c6fb9ec2468fb76ad3d2dd1d/sakura-7b-qwen2.5-v1.0-iq4xs.gguf'; Source = 'SakuraLLM (Hugging Face, CC BY-NC-SA 4.0);日→中特化';
       Sha256 = '30e82ca13bc1e321f4f7b77459d533d483d46200522dc5c31eef9d2f2a6335d6' }
    $items += @{ Name = 'Sakura-1.5B-Qwen2.5-v1.0-F16.gguf'; Size = '3.56 GB'; Dest = (Join-Path $models 'Sakura-1.5B-Qwen2.5-v1.0-F16.gguf');
       Url = 'https://huggingface.co/SakuraLLM/Sakura-1.5B-Qwen2.5-v1.0-GGUF/resolve/959df27e558735027e0e56b65cda800871973538/sakura-1.5b-qwen2.5-v1.0-fp16.gguf'; Source = 'SakuraLLM (Hugging Face, CC BY-NC-SA 4.0);日→中特化小模型';
       Sha256 = 'eb0f76c7d52710329c4071d8fbbef5c51f9a4f047c066472e6c7063a53e45764' }
}
if ($WithCandidates) {
    $cands = @(
        @('Seed-X-PPO-7B-Q4_K_M.gguf', '4.56 GB', 'https://huggingface.co/mradermacher/Seed-X-PPO-7B-GGUF/resolve/c9166f2c073d3219fe22e1f35910a41e666e971f/Seed-X-PPO-7B.Q4_K_M.gguf', 'mradermacher (Hugging Face);ByteDance Seed-X,OpenMDW', '9202183d5b52a1d80271cfdf20e4bf98fb7430c797a225581cd226ba7ba3cdb1'),
        @('LMT-60-4B-Q4_K_M.gguf', '2.72 GB', 'https://huggingface.co/mradermacher/LMT-60-4B-GGUF/resolve/fd76914adb08e4aa076ef7044e0f3b54d5987a5d/LMT-60-4B.Q4_K_M.gguf', 'mradermacher (Hugging Face);NiuTrans LMT-60,Apache-2.0', '6ab245a422b854b4582401c1240b64c35899fe1c816af059aa23b38ac9707bb4'),
        @('LMT-60-8B-Q4_K_M.gguf', '5.03 GB', 'https://huggingface.co/mradermacher/LMT-60-8B-GGUF/resolve/5db5f369e614a63c90c4b3100585ef15a0fe53d0/LMT-60-8B.Q4_K_M.gguf', 'mradermacher (Hugging Face);NiuTrans LMT-60,Apache-2.0', '2661b6d2ffe0c97df9340851aec6b8a97e92d2619d25d4c13596e4b9fb5bcfc7'),
        @('TranslateGemma-4B-it-Q4_K_M.gguf', '2.49 GB', 'https://huggingface.co/mradermacher/translategemma-4b-it-GGUF/resolve/d37868f759d2f817a43d28dd80cf9f982ffa62fd/translategemma-4b-it.Q4_K_M.gguf', 'mradermacher (Hugging Face);Google TranslateGemma,Gemma 授權', '81200d03e843d2ec1ece6eeafe7d13cb6e5211e1fcd336ade55790b683a08330'),
        @('TranslateGemma-12B-it-Q4_K_M.gguf', '7.30 GB', 'https://huggingface.co/mradermacher/translategemma-12b-it-GGUF/resolve/41d7c8aa650b26791225ad9d0597e674e680da96/translategemma-12b-it.Q4_K_M.gguf', 'mradermacher (Hugging Face);Google TranslateGemma,Gemma 授權', 'b7aac4b4be7ab0c49b6556c29c4467e74313df7f1e95d9f9676bb2adf0afa528')
    )
    foreach ($c in $cands) {
        $items += @{ Name = $c[0]; Size = $c[1]; Dest = (Join-Path $models $c[0]); Url = $c[2]; Source = $c[3]; Sha256 = $c[4] }
    }
}
# sherpa-onnx 的 tar.bz2 模型包:下載後解壓到 models\<目錄名>(Windows 10 起內建 tar.exe 支援 bz2)
# 指定 System32 的 tar:PATH 有 Git 的 usr\bin 時 tar.exe 會是 MSYS 版,把 D:\ 當成遠端主機而失敗
$tarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
$archives = @()
if ($WithReazonSpeech) {
    $archives += @{ Name = 'sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01'; Size = '713 MB';
       Source = 'k2-fsa/sherpa-onnx (GitHub Releases);reazon-research/reazonspeech-k2-v2,Apache-2.0';
       Sha256 = 'e0981d0d5d7b446d41010831b59091ebf57d2aa7b79980f67ca37af460b5842d';
       Files = @(@('encoder-epoch-99-avg-1.int8.onnx', '2c7bd08a8a99f9ddd0d9e458456577b1f6279214e51426f114f9eced44c54e1d'),
                 @('decoder-epoch-99-avg-1.int8.onnx', '8f0bff94d38797b03b762634ed03211a8e303d06cc4603cdd0cf4199d6eb1485'),
                 @('joiner-epoch-99-avg-1.int8.onnx', '49cc7ea1d3d35a40a27442db5e89996da64bf0e683a903dce76e99e57a12e4de'),
                 @('tokens.txt', '2c3ac659818a48a0c04010e0593bbc4d7c8a24a054340b01131499c05fd52def')) }
}
if ($WithParakeet) {
    $archives += @{ Name = 'sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8'; Size = '482 MB';
       Source = 'k2-fsa/sherpa-onnx (GitHub Releases);NVIDIA parakeet-tdt-0.6b-v2,CC-BY-4.0';
       Sha256 = '157c157bc51155e03e37d2466522a3a737dd9c72bb25f36eb18912964161e1ad';
       Files = @(@('encoder.int8.onnx', 'a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab'),
                 @('decoder.int8.onnx', 'b6bb64963457237b900e496ee9994b59294526439fbcc1fecf705b31a15c6b4e'),
                 @('joiner.int8.onnx', '7946164367946e7f9f29a122407c3252b680dbae9a51343eb2488d057c3c43d2'),
                 @('tokens.txt', 'ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d')) }
}
foreach ($a in $archives) {
    $items += @{ Name = "$($a.Name).tar.bz2"; Size = $a.Size; Dest = (Join-Path $models "$($a.Name).tar.bz2");
       Url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$($a.Name).tar.bz2"; Source = $a.Source;
       Extracted = (Join-Path $models $a.Name); Sha256 = $a.Sha256; Files = $a.Files }
}
if ($With7B) {
    $items += @{ Name = 'Hy-MT2-7B-Q4_K_M.gguf'; Size = '4.62 GB'; Dest = (Join-Path $models 'Hy-MT2-7B-Q4_K_M.gguf');
       Url = 'https://huggingface.co/tencent/Hy-MT2-7B-GGUF/resolve/ab8472660ac61fac25f1af43fac2599d52a8a775/Hy-MT2-7B-Q4_K_M.gguf'; Source = 'tencent/Hy-MT2-7B-GGUF (Hugging Face)';
       Sha256 = '9f96256500f3fc1ab4d64336b58f52a949a95ad7516b0c229476eef782f9f77b' }
}

function Test-Sha256($Path, $Expected) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash -eq $Expected
}

Write-Host "將下載以下檔案到 $root :"
foreach ($i in $items) {
    $state = if ($i.Extracted -and (Test-Path (Join-Path $i.Extracted 'tokens.txt'))) { if ($Verify) { '已解壓,校驗' } else { '已存在,略過' } }
             elseif (Test-Path $i.Dest) { if ($Verify) { '已存在,校驗' } else { '已存在,略過' } }
             else { '待下載' }
    Write-Host ("  [{0}] {1,-32} {2,8}  來源:{3}" -f $state, $i.Name, $i.Size, $i.Source)
}
if ($ListOnly) { exit 0 }

$bad = @()
foreach ($i in $items) {
    if ($i.Extracted -and (Test-Path (Join-Path $i.Extracted 'tokens.txt'))) {
        # 已解壓:-Verify 時逐一校驗 app 實際使用的檔案(壓縮檔解壓後已刪除)
        if ($Verify) {
            foreach ($f in $i.Files) {
                $p = Join-Path $i.Extracted $f[0]
                if (-not (Test-Path -LiteralPath $p) -or -not (Test-Sha256 $p $f[1])) { $bad += $p }
            }
        }
        continue
    }
    if (Test-Path $i.Dest) {
        # 壓縮檔接著要解壓成模型或執行檔,每次都校驗;其他檔案指定 -Verify 才校驗
        if (($Verify -or $i.Name -match '\.(zip|tar\.bz2)$') -and -not (Test-Sha256 $i.Dest $i.Sha256)) { $bad += $i.Dest }
        continue
    }
    Write-Host "下載 $($i.Name) ..."
    $tmp = "$($i.Dest).part"
    & curl.exe -L --retry 3 --progress-bar -o $tmp $i.Url
    if ($LASTEXITCODE -ne 0) { throw "下載失敗:$($i.Url)" }
    if (-not (Test-Sha256 $tmp $i.Sha256)) {
        Remove-Item -Force $tmp
        throw "SHA256 不符,已刪除下載檔:$($i.Name)(來源 $($i.Url))"
    }
    Move-Item -Force $tmp $i.Dest
}
# 回傳 zip 內每個檔案與 $Dir 內對應檔 SHA256 缺漏或不符的完整路徑
function Get-ZipMismatch($Zip, $Dir) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    # .NET 的工作目錄可能與 PowerShell 位置不同,先轉成絕對路徑
    $zipFull = (Resolve-Path -LiteralPath $Zip).ProviderPath
    $out = @()
    $z = [IO.Compression.ZipFile]::OpenRead($zipFull)
    try {
        foreach ($e in $z.Entries) {
            if (-not $e.Name) { continue }  # 目錄條目
            $p = Join-Path $Dir $e.FullName
            if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { $out += $p; continue }
            $s = $e.Open()
            try { $h = (Get-FileHash -Algorithm SHA256 -InputStream $s).Hash } finally { $s.Dispose() }
            if (-not (Test-Sha256 $p $h)) { $out += $p }
        }
    } finally { $z.Dispose() }
    $out
}
# 實際當 sidecar 用的 llama 執行檔已解壓時,與 zip 內容比對(zip 本身上面已校驗)
if ($Verify -and (Test-Path (Join-Path $bin 'llama\llama-server.exe'))) {
    $bad += @(Get-ZipMismatch (Join-Path $bin "llama-$llamaBuild-bin-win-vulkan-x64.zip") (Join-Path $bin 'llama'))
}
if ($bad) {
    throw "以下已存在的檔案 SHA256 不符,請刪除後重跑:`n  $($bad -join "`n  ")"
}
if ($Verify) { Write-Host '已存在的檔案(含已解壓的專用模型與 llama 執行檔)SHA256 全部相符。' }

foreach ($i in $items | Where-Object { $_.Extracted }) {
    # 以 tokens.txt 當完成標記:解壓中斷留下的半套目錄不算完成
    if (Test-Path (Join-Path $i.Extracted 'tokens.txt')) { continue }
    Write-Host "解壓 $($i.Name) ..."
    & $tarExe -xjf $i.Dest -C $models
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
