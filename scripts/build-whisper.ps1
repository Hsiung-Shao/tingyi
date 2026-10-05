# 本機以 CUDA 編譯 whisper.cpp 的 whisper-server(官方 release 無 Windows Vulkan 版)。
# 需求:git、CMake、Ninja(pip install --user ninja)、MSVC 2022、CUDA Toolkit 12.8+(RTX 50 系列 sm_120 需 12.8+;
# 本機 13.1 目錄只有 stub 沒有 nvcc)。不需要 CUDA 的 Visual Studio 整合:走 Ninja + vcvars64。
# 用法:powershell -ExecutionPolicy Bypass -File scripts\build-whisper.ps1 [-Tag b5130] [-CudaArch 120] [-CudaVersion 12.8]
param(
    [string]$Tag = "b5130",
    [string]$CudaArch = "120",
    [string]$CudaVersion = "12.8"
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$src = Join-Path $repoRoot 'data\src\whisper.cpp'
$outDir = Join-Path $repoRoot 'data\bin\whisper'
$cuda = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CudaVersion"
$nvcc = Join-Path $cuda 'bin\nvcc.exe'
if (-not (Test-Path $nvcc)) { throw "找不到 nvcc:$nvcc(請確認 CUDA Toolkit $CudaVersion 已安裝)" }
$vcvars = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if (-not (Test-Path $vcvars)) { $vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" }
if (-not (Test-Path $vcvars)) { throw "找不到 vcvars64.bat" }
$ninja = Get-Command ninja -ErrorAction SilentlyContinue
if (-not $ninja) {
    $cand = Join-Path $env:APPDATA 'Python\Python312\Scripts\ninja.exe'
    if (Test-Path $cand) { $env:PATH = (Split-Path $cand) + ';' + $env:PATH } else { throw "找不到 ninja(pip install --user ninja)" }
}

New-Item -ItemType Directory -Force (Split-Path $src) | Out-Null
if (-not (Test-Path (Join-Path $src 'CMakeLists.txt'))) {
    Write-Host "clone whisper.cpp $Tag ..."
    & git clone --depth 1 --branch $Tag https://github.com/ggml-org/whisper.cpp $src
    if ($LASTEXITCODE -ne 0) { throw "git clone 失敗" }
}

$build = Join-Path $src 'build-cuda-ninja'
$env:CUDACXX = $nvcc
$env:CUDA_PATH = $cuda
$env:PATH = (Join-Path $cuda 'bin') + ';' + 'C:\Program Files (x86)\Microsoft Visual Studio\Installer;' + $env:PATH
$cmakeArgs = "-S `"$src`" -B `"$build`" -G Ninja -DCMAKE_BUILD_TYPE=Release -DGGML_CUDA=ON -DCMAKE_CUDA_ARCHITECTURES=$CudaArch `"-DCMAKE_CUDA_COMPILER=$nvcc`" -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_EXAMPLES=ON -DWHISPER_SDL2=OFF -DGGML_CCACHE=OFF"
Write-Host "cmake configure(Ninja + MSVC,CUDA $CudaVersion,arch $CudaArch)..."
& cmd /c "`"$vcvars`" >nul && cmake $cmakeArgs"
if ($LASTEXITCODE -ne 0) { throw "cmake configure 失敗" }

Write-Host "cmake build whisper-server(Release)..."
& cmd /c "`"$vcvars`" >nul && cmake --build `"$build`" --target whisper-server --parallel"
if ($LASTEXITCODE -ne 0) { throw "cmake build 失敗" }

New-Item -ItemType Directory -Force $outDir | Out-Null
$exe = Get-ChildItem -Recurse $build -Filter 'whisper-server.exe' | Select-Object -First 1
if (-not $exe) { throw "找不到 whisper-server.exe" }
Copy-Item -Force $exe.FullName $outDir
Get-ChildItem -Recurse $build -Filter '*.dll' -ErrorAction SilentlyContinue | Copy-Item -Force -Destination $outDir
# CUDA 執行期 DLL(讓 exe 在沒裝 Toolkit 的機器也能跑)
foreach ($pat in @('cudart64_*.dll', 'cublas64_*.dll', 'cublasLt64_*.dll')) {
    Get-ChildItem (Join-Path $cuda 'bin') -Filter $pat -ErrorAction SilentlyContinue | Copy-Item -Force -Destination $outDir
}
Write-Host "完成:$outDir"
Get-ChildItem $outDir | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}} | Format-Table -AutoSize
