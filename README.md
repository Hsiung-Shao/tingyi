# 聽譯(tingyi)

輕量 Windows 桌面應用:擷取電腦正在播放的英、日、韓語音,在本機即時辨識並翻成台灣繁體中文,以懸浮字幕顯示。預設完全離線,不需要任何 API key。

> 目前狀態:**技術驗證週**(見 `docs/plan-tech-validation.md` 與 `docs/validation-report.md`)。

## 架構

```
音訊來源(WASAPI loopback:系統全部聲音,或像 OBS 選一個視窗、擷取該程式含子程序,執行中可切換)
  → 16 kHz mono → Silero VAD 切段 → 音量正規化
  → 辨識:SenseVoice-Small int8(sherpa-onnx,CPU;也負責講話中的即時預覽)
        或 專用模型(CPU:日文 ReazonSpeech k2 v2、英文 Parakeet TDT v2,其餘 SenseVoice)
        或 Whisper large-v3-turbo(whisper-server sidecar,CUDA GPU,選配)
  → 清理:幻聽式重複壓縮 + 使用者辨識取代表
  → 依語言判斷句子是否說完,未完則等下一段合併
  → Hy-MT2(llama-server sidecar,Vulkan GPU)串流翻譯 + 台灣用語後處理
  → Tauri 2 事件 → React 主視窗(紀錄、指標)+ 懸浮字幕視窗(置頂、透明、滑鼠穿透)

網頁翻譯(Chrome 擴充 `web/extension`)
  → native messaging host(同一個 tingyi.exe,`com.jerry.tingyi.host`)讀 runtime.json;聽譯沒開就背景喚起
  → 與字幕共用同一個 llama-server(-np 4:slot 0 字幕、1 回修、2–3 網頁;api key 防其他網頁偷用)
```

| 目錄 | 內容 |
|---|---|
| `crates/tingyi-core` | 純 Rust 核心:`audio/`(擷取、音訊來源、重取樣)、`vad`、`asr`、`route`(依語言路由)、`model_download`(專用模型下載)、`cleanup`、`eval`(CER/WER)、`mt`、`postprocess`、`pipeline`、`metrics`、`llama`(子程序管理)、`translate_server`(字幕與網頁共用的翻譯伺服器)、`paths` |
| `crates/tingyi-bench` | 量測 CLI:`devices` / `capture-probe` / `vad` / `asr` / `asr-file` / `asr-eval` / `mt` / `live` / `serve` / `download-model` |
| `src-tauri` | Tauri 2 後端:命令、狀態、事件轉發;網頁翻譯的 native host、runtime.json、系統匣(`web.rs`、`native_host.rs`) |
| `src` | React 19 + Tailwind 4 前端:`App.tsx` 主視窗、`Overlay.tsx` 懸浮字幕 |
| `web` | 網頁翻譯擴充(原 agent-translator,以 git subtree 併入):`extension/`(Chrome MV3)、`packages/core`、`bridge/`(選用的 CLI agent MCP server,目前未接進聽譯);獨立的 pnpm workspace,`pnpm --dir web test` |
| `scripts/download-models.ps1` | 下載模型與 llama-server 到資料目錄(`-WithWhisper` 加 Whisper 模型;`-WithReazonSpeech`、`-WithParakeet` 加專用辨識模型;安裝版也能在主視窗「辨識引擎」下方直接下載專用模型;Hugging Face 網址固定到 commit,每個檔案校驗 SHA256,`-Verify` 重驗已存在的檔案,`-DataDir` 指定資料目錄) |
| `scripts/fetch-eval.py` | 從 Hugging Face parquet 抓 CER 評測集(ReazonSpeech test、JSUT、FLEURS ja/en/ko)到 `data/eval/` |
| `scripts/build-whisper.ps1` | 以 CUDA 自行編譯 whisper.cpp 的 whisper-server(官方無 Windows Vulkan 版;需 CUDA 12.8+、Ninja) |
| `scripts/tts-smoke.ps1` | 用 Windows TTS 透過喇叭唸句子,自動化測試整條管線 |
| `scripts/shot-windows.ps1` | 以 PrintWindow 截取 app 各視窗(含被遮住的),驗 UI 用 |
| `data/`(gitignore) | 模型、llama-server、測試音檔 |

## 環境需求

- Windows 10/11、Rust 1.92+(msvc)、Node 24、pnpm 10、WebView2
- NVIDIA / AMD / Intel 顯示卡(Vulkan);預設模型組合約需 2.5 GB 顯示記憶體

## 安裝與執行

```powershell
pnpm install
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 -With7B -With1_8B_Q4   # 約 2.2 GB(加 7B 為 7.5 GB)
pnpm tauri dev
```

資料目錄解析順序:環境變數 `TINGYI_DATA_DIR` → 專案內 `data/`(存在時)→ `%LOCALAPPDATA%\tingyi`。下載腳本不看專案內 `data/`(預設 `TINGYI_DATA_DIR` → `%LOCALAPPDATA%\tingyi`),要下載到那裡請加 `-DataDir data`。

## 量測

```powershell
cargo build -p tingyi-bench
.\target\debug\tingyi-bench.exe devices
.\target\debug\tingyi-bench.exe asr-file data\test_wavs\en.wav data\test_wavs\ja.wav
.\target\debug\tingyi-bench.exe asr-file --engine whisper data\test_wavs\ja.wav     # 需先 build-whisper + -WithWhisper
.\target\debug\tingyi-bench.exe live --seconds 120 --asr whisper --record bench-out\segs --out bench-out\live-whisper.jsonl
.\target\debug\tingyi-bench.exe mt --sentences docs\bench\sentences-30.tsv --models Hy-MT2-1.8B-Q8_0 Hy-MT2-7B-Q4_K_M --out bench-out\mt.jsonl
.\target\debug\tingyi-bench.exe live --seconds 180 --model Hy-MT2-1.8B-Q8_0 --out bench-out\live.jsonl
.\target\debug\tingyi-bench.exe capture-probe --source app:chrome.exe --seconds 30    # 擷取封包、靜音、不連續次數
python scripts\fetch-eval.py                                                           # 評測集(約 400 MB)
.\target\debug\tingyi-bench.exe asr-eval --engines sensevoice routed whisper           # 逐句 CER/WER、延遲
.\target\debug\tingyi-bench.exe asr-eval --stream --engines routed --pre-pad-ms 400    # 串流(含 VAD 與修補)A/B
```

另開一個終端用 TTS 自動供音:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\tts-smoke.ps1 -Lines docs\bench\tts-en.txt
```

## 測試

```powershell
cargo test --workspace
pnpm build
```

## 授權與模型

- 程式碼:MIT
- Hy-MT2(tencent,Apache-2.0)、SenseVoice-Small(sherpa-onnx 轉出,Apache-2.0)、Silero VAD(MIT)、llama.cpp(MIT)
- ReazonSpeech k2 v2(reazon-research,Apache-2.0)、Parakeet TDT 0.6B v2(NVIDIA,CC-BY-4.0,需標示出處)
- 內嵌 [tao](https://github.com/tauri-apps/tao) 0.37.1(Apache-2.0)於 `third_party/tao-0.37.1/`,只修一處會讓正式版偶發 panic 的 assert(見該目錄 `TINGYI-PATCH.md`)
- 評測集只供本機量測、不隨 repo 散布:ReazonSpeech 測試集、JSUT basic5000、Google FLEURS(各自授權)
