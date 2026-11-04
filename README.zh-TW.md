# 聽譯(tingyi)

[English](README.md) | [繁體中文](README.zh-TW.md)

輕量 Windows 桌面應用:擷取電腦正在播放的英、日、韓語音,在本機即時辨識並翻成台灣繁體中文,以懸浮字幕顯示。預設完全離線,不需要任何 API key。

另附一個 Chrome 擴充,用同一個本機翻譯模型做雙語網頁翻譯:

- 網頁原地翻譯,可設定常用網站開啟時自動翻譯(例如 Reddit、X)。
- 翻譯 YouTube 字幕,只顯示譯文。
- YouTube 影片**沒有字幕**時,改由聽譯聽瀏覽器的聲音,把即時字幕顯示在播放器裡。

## 安裝(一般使用者)

1. 到 [Releases](../../releases) 下載:
   - `tingyi_<版本>_x64-setup.exe`:Windows 安裝檔
   - `tingyi-extension-<版本>.zip`:未封裝的 Chrome 擴充
2. 執行安裝檔,裝完打開聽譯一次(它會向 Chrome 註冊,擴充才連得到)。
3. 主視窗「翻譯模型」區若顯示未下載,先下載 llama.cpp 與 Hy-MT2 1.8B。
4. 解壓擴充 zip,打開 `chrome://extensions`,開啟右上角「開發人員模式」,按「載入未封裝項目」並選解壓出來的資料夾。
5. 開任一網頁,用擴充 popup(或 `Alt+A`)翻譯。popup 的「這個網站」可以設定:開啟時自動翻譯、翻譯影片字幕、影片沒有字幕時用聽譯翻譯。

說明:

- 擴充需要翻譯時會自動在背景啟動聽譯。主視窗的「網頁翻譯」卡片可以看狀態、啟動或停止。
- 「沒有字幕時用聽譯翻譯」聽的是整個瀏覽器的聲音,其他分頁有聲音也會被辨識。
- 字幕預設譯成台灣繁體中文。主視窗「翻譯模型」區可調整「翻譯成」與「譯文轉換」(簡體輸出怎麼轉)。

## 架構

```
音訊來源(WASAPI loopback:系統全部聲音,或像 OBS 選一個視窗、擷取該程式含子程序,執行中可切換)
  → 16 kHz mono → Silero VAD 切段 → 音量正規化
  → 辨識:SenseVoice-Small int8(sherpa-onnx,CPU;也負責講話中的即時預覽)
        或 專用模型(CPU:日文 ReazonSpeech k2 v2、英文 Parakeet TDT v2,其餘 SenseVoice)
        或 Whisper large-v3-turbo(whisper-server sidecar,CUDA GPU,選配)
  → 清理:幻聽式重複壓縮 + 使用者辨識取代表
  → 依語言判斷句子是否說完,未完則等下一段合併
  → Hy-MT2(llama-server sidecar,Vulkan GPU)串流翻譯 + 簡轉台灣繁體後處理
  → Tauri 2 事件 → React 主視窗(紀錄、指標)+ 懸浮字幕視窗(置頂、透明、滑鼠穿透)

網頁翻譯(Chrome 擴充 `web/extension`)
  → native messaging host(同一個 tingyi.exe,`com.jerry.tingyi.host`)讀 runtime.json;聽譯沒開就背景喚起
  → 與字幕共用同一個 llama-server(-np 4:slot 0 字幕、1 回修、2–3 網頁;api key 防其他網頁偷用)
  → YouTube 無字幕:connectNative → 聽譯的 named pipe(只限本人)→ 字幕畫在播放器內的 closed shadow root
```

| 目錄 | 內容 |
|---|---|
| `crates/tingyi-core` | 純 Rust 核心:擷取與音訊來源、VAD、辨識、依語言路由、模型下載、清理、CER/WER 評測、翻譯提示、後處理、管線、指標、llama-server 管理、共用翻譯伺服器 |
| `crates/tingyi-bench` | 量測 CLI:`devices` / `capture-probe` / `vad` / `asr` / `asr-file` / `asr-eval` / `mt` / `live` / `serve` / `download-model` |
| `src-tauri` | Tauri 2 後端:命令、狀態、事件轉發;native host、runtime.json、系統匣、YouTube 即時字幕的 named pipe |
| `src` | React 19 + Tailwind 4 前端:`App.tsx` 主視窗、`Overlay.tsx` 懸浮字幕 |
| `web` | 網頁翻譯擴充(原 agent-translator,以 git subtree 併入):`extension/`(Chrome MV3)、`packages/core`、`bridge/`(選用的 CLI agent MCP server,目前未接進聽譯);獨立的 pnpm workspace |
| `scripts/download-models.ps1` | 下載模型與 llama-server 到資料目錄(Hugging Face 網址固定到 commit,每個檔案校驗 SHA256) |
| `data/`(gitignore) | 模型、llama-server、測試音檔 |

## 開發

環境需求:Windows 10/11、Rust 1.92+(msvc)、Node 24、pnpm 10、WebView2、支援 Vulkan 的顯示卡(預設模型約需 2.5 GB 顯示記憶體)。

```powershell
pnpm install
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 -With7B -With1_8B_Q4
pnpm tauri dev
```

資料目錄解析順序:環境變數 `TINGYI_DATA_DIR` → 專案內 `data/`(存在時)→ `%LOCALAPPDATA%\tingyi`。

擴充:

```powershell
pnpm --dir web install
pnpm --dir web --filter @atx/extension build   # 產物在 web/extension/dist(載入未封裝項目)
```

測試:

```powershell
cargo test --workspace
pnpm --dir web test
pnpm build
```

量測工具與驗證紀錄見 `docs/validation-report.md`。

## 授權與模型

- 程式碼:MIT
- 模型:Hy-MT2(tencent,Apache-2.0)、SenseVoice-Small(sherpa-onnx 轉出,Apache-2.0)、Silero VAD(MIT)、llama.cpp(MIT)、ReazonSpeech k2 v2(reazon-research,Apache-2.0)、Parakeet TDT 0.6B v2(NVIDIA,CC-BY-4.0,需標示出處)
- 內嵌 [tao](https://github.com/tauri-apps/tao) 0.37.1(Apache-2.0)於 `third_party/tao-0.37.1/`,只修一處會讓正式版偶發 panic 的 assert(見該目錄 `TINGYI-PATCH.md`)
- 擴充的簡轉繁使用 [opencc-js](https://github.com/nk2028/opencc-js)(MIT / Apache-2.0)
- 評測集只供本機量測、不隨 repo 散布:ReazonSpeech 測試集、JSUT basic5000、Google FLEURS(各自授權)
