---
name: verify
description: 聽譯 tingyi(Tauri 2 Windows 桌面 app,即時字幕翻譯)的實跑驗證。啟動 `pnpm tauri dev`,以 WebView2 遠端除錯埠(CDP)在頁面內操作主視窗,用自己啟動的 TTS 程式供音,驗證指定程式音訊來源、執行中切換與字幕產生;留截圖、run.log、exit_code.txt。宣稱 UI/擷取/管線行為正確前、/handoff-check 第 7 段使用。
---

# 聽譯 verify

表面:Tauri 2 桌面程式(主視窗 `main` + 懸浮字幕 `overlay`),前端 Vite dev server `http://localhost:1420/`。
驅動:WebView2 環境變數 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9339` 開 CDP,在頁面內執行 JS(點按鈕、改下拉、讀 DOM)。**不送任何 OS 鍵鼠輸入**,不搶使用者前景視窗。截圖用 `scripts/shot-windows.ps1`(PrintWindow,視窗被蓋住也拍得到)。

## Launch

一鍵(含 Doctor、Drive、Evidence、Cleanup):

```bash
python .claude/skills/verify/scripts/verify_audio_source.py
python .claude/skills/verify/scripts/verify_model_download.py
python .claude/skills/verify/scripts/verify_download_script.py --existing D:\codeproject\translate\data
python .claude/skills/verify/scripts/verify_release_build.py      # 先 pnpm tauri build;在 main 的工作目錄跑
node .claude/skills/verify/scripts/verify_web_translate.mjs        # 網頁翻譯(擴充 + 共用翻譯伺服器)
node .claude/skills/verify/scripts/verify_youtube_listen.mjs       # YouTube 沒字幕改用聽譯(播放器內字幕)
```

第五支驗網頁翻譯(見 `features/web-translate.md`):`pnpm tauri dev` 啟動聽譯(`TINGYI_REGISTER_HOST=1` 讓開發版寫 native host 註冊),主視窗以 1.8B 開始字幕,再用 ms-playwright 的 Chromium 載入 `web/extension/dist`(腳本會先建置)。檢查:擴充自動取得端點與 slots `[2,3]`、網頁與字幕只有一個 llama-server、沒帶 api key 被拒、fixture 頁翻好、停止字幕後網頁照翻、強制結束聽譯後 llama-server 跟著結束且擴充自動喚起聽譯恢復翻譯。證據在 `evidence/<時間>-web/`。在 Claude 環境內登錄檔與 LOCALAPPDATA 會被重導(Chromium 與聽譯同在重導範圍內,所以旅程本身一致),真實環境要另由使用者從檔案總管驗。

第二支驗專用模型的 app 內下載(見 `features/routed-models.md`):自己架假 Hugging Face(本機 Range 伺服器,提供 `data/models` 的真實檔、限速),`TINGYI_DATA_DIR` 指向證據目錄裡的臨時資料目錄,結束後刪掉臨時資料目錄(只留檔案清單)。選項 `--source-data`、`--rate-mbps`。

第三支驗開發用下載腳本 `scripts/download-models.ps1`(真網路,約 3.4 GB,不啟動 app):乾淨資料目錄下載必要項目與日英專用模型包、Python 獨立重算 SHA256、`-Verify`、竄改一個位元組必須失敗且錯誤訊息只點名該檔、竄改已解壓的模型與 llama DLL 也必須被 `-Verify` 抓到,且照訊息「請刪除:」的路徑刪除後重跑可恢復;`--existing` 再對既有資料目錄的全部選配項目跑 `-Verify`(先 `-ListOnly`,有「待下載」就不跑)。結束時刪掉下載的資料(只留檔案清單);腳本會把 llama-server 複製到本 worktree 的 `src-tauri\binaries`(已 gitignore)。

第四支驗正式版打包(`pnpm tauri build` 之後):不安裝、不動使用者已安裝的版本,直接啟動 `target\release\tingyi.exe`(CDP,正式版頁面是 `http://tauri.localhost/`);檢查安裝檔與執行檔修改時間相差 120 秒內(同一次建置)、執行檔是 GUI 子系統、執行檔含 tao 修補(位元組有 `(tingyi patch)`,沒有代表 `[patch.crates-io]` 沒生效)、專用模型顯示已下載、專用模型 + 本機翻譯可開始且 llama-server 子程序啟動(PID 取自 app log)、鎖定日文、log 有 `routed[reazonspeech](`、停止後 llama-server 由 app 結束、溫和關閉主視窗(WM_CLOSE)後 app 自行結束且 returncode 0、關閉後自己的 tingyi / llama-server PID 無殘留;另有壓力回歸檢查(`--stress-rounds`,預設 20 輪「清單列舉 + PrintWindow 截圖」同時進行,app 不得當掉;原版 tao 會偶發 panic,已內嵌修補,見 `third_party/tao-0.37.1/TINGYI-PATCH.md`);收尾會統計 app.log 中修補路徑觸發次數(`tao_patch_hits`,只記錄不判定)。正式版的資料目錄是編譯時的 workspace `data/`(存在時),所以要在 main 的工作目錄(`D:\codeproject\translate`)跑;`--data-dir <空目錄>` 是破壞對照(必須失敗)。介面設定不存檔,不會改到使用者平常的設定。

選項:`--data-dir <模型資料目錄>`(預設 `D:\codeproject\translate\data`;worktree 沒有 `data/`,不設會退回 `%LOCALAPPDATA%\tingyi` 而找不到模型)、`--launch-timeout 600`。

腳本內部的啟動方式(手動除錯時照做):

```powershell
$env:TINGYI_DATA_DIR = 'D:\codeproject\translate\data'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9339'
pnpm tauri dev
```

「可用」的判準不是行程在跑,而是:CDP `http://127.0.0.1:9339/json` 出現 url 為 `http://localhost:1420/` 的 page,且頁面上「開始」按鈕存在且未 disabled(代表 `get_info` 已回來、模型檔就緒)。首次編譯約 1–5 分鐘,之後約 50 秒。

## Doctor(唯讀,決定值不值得驅動)

- 1420、9339 埠都沒被占用,且沒有 `tingyi.exe` 在跑 → 否則**拒絕雙開**(安裝版 tingyi 與 dev 版共用 WebView2 使用者資料夾,雙開會互相干擾)。9222 常被 Chrome 除錯埠占用,所以不用。
- `<data-dir>\models\silero_vad.onnx` 與 `<data-dir>\models\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17\model.int8.onnx` 存在。

## Drive

穩定把手(都在 `src/App.tsx`、`src/AudioSourcePicker.tsx`):

| 對象 | 選擇器 |
|---|---|
| 開始 / 停止 | `button` 文字剛好是 `開始` / `停止` |
| 翻譯開關 | 文字含 `啟用本機翻譯` 的 `label` 內的 checkbox |
| 音訊來源下拉 | `select[aria-label="音訊來源"]`,值 `system` 或 `app:<pid>` |
| 重新整理程式清單 | `button[aria-label="重新整理程式清單"]` |
| 字幕列 | `[data-sentence-id]`(`data-lang`),原文在 `[data-role=source]` |
| 來源狀態(只讀觀察) | `window.__TAURI_INTERNALS__.invoke('get_audio_source')` |

React 受控 `select` 要用原生 setter + `change` 事件才會觸發 onChange(見 `scripts/verify_audio_source.py` 的 `JS_HELPERS.choose`)。

單次查詢頁面:`python .claude/skills/verify/scripts/cdp.py "document.title"`(app 需已用上面方式啟動)。

## Evidence

`evidence/<yyyyMMdd-HHmmss>/`:`run.log`(每一步與 PASS/FAIL)、`exit_code.txt`(0/1)、`app.log`(tauri dev 與 Rust log)、`result.json`(各階段字幕內容)、`1-tts/`、`2-silent/`、`3-back/` 截圖、`tts-lines.txt`、`silence.wav`。證據目錄已列入 `.gitignore`(截圖可能含個人畫面),不入庫。

驗證完跑:

```bash
python C:\Users\jerry\.claude\scripts\evidence_gate.py .claude/skills/verify/evidence/<時間> --source-root .
```

## Cleanup

腳本只關閉自己啟動的行程:`taskkill /PID <pid> /T /F` 依序關 TTS、靜音播放器、`pnpm tauri dev` 的 cmd 外殼(連同 node/cargo/tingyi.exe 子樹),再以 PID 確認 `tingyi.exe` 已結束,仍活著才依 PID 補關。**不按名稱殺**(使用者可能同時開著其他 powershell 或安裝版 tingyi)。證據不刪。

## Helpers

- `scripts/cdp.py`:最小 CDP 用戶端(`connect(port, url_prefix)` → `Page.eval(js)`;也可命令列單次執行)。依賴 Python `websockets`。
- `scripts/verify_audio_source.py`:指定程式音訊來源旅程(見 `features/audio-source.md`)。
- `scripts/tts-window.ps1`:驗證用 TTS,同一程序開一個最小化、不搶前景的視窗(標題 `-Title`),讓清單以 `[powershell.exe]: <標題>` 列出。
- `scripts/verify_model_download.py`:專用模型下載旅程(見 `features/routed-models.md`)。
- `scripts/verify_download_script.py`:開發用下載腳本的 SHA256 校驗(見上方說明)。
- `scripts/verify_release_build.py`:正式版執行檔與安裝檔的實跑(見上方說明)。
- `scripts/verify_web_translate.mjs`:網頁翻譯旅程(Node 24 內建 WebSocket;見 `features/web-translate.md`)。
- `scripts/verify_youtube_listen.mjs`:YouTube 沒字幕改用聽譯(本機 HTTPS 樣本頁冒充 www.youtube.com:`--host-resolver-rules` + openssl 自簽憑證 + `--ignore-certificate-errors`;TTS 用 Zira)。證據在 `evidence/<時間>-yt/`。三支旅程都用暫存資料目錄(junction 指到 `--data` 的 bin、models),不寫進安裝版的 settings/runtime。跑的時候不要碰鍵盤滑鼠:開發版主視窗會跳到最前面接住輸入。
- `scripts/bench-shared-server.py`(專案根目錄 `scripts/`):共用伺服器的字幕首字延遲與顯示記憶體量測。
- 專案腳本:`scripts/tts-smoke.ps1`(TTS 供音)、`scripts/shot-windows.ps1`(截圖)。

## 破壞對照紀錄

2026-10-25:把 `capture.rs` 的世代號檢查改成 `if false && ...`(UI 顯示已切換但擷取不換),驗證在「切到靜音播放器後 12 秒內沒有新字幕」FAIL、exit 1(證據 `evidence/20261025-132442-neg/negative/`);以事前備份逐位元組還原(sha256 5e2d2cf3…)後恢復 PASS。

2026-10-25:把 `model_download.rs` 的 `install(...)` 換成不執行(下載完不搬進正式目錄),`verify_model_download.py` 在「下載完成顯示「已下載」」FAIL、exit 1(證據 `evidence/20261025-164513-neg/negative/`);以事前備份逐位元組還原(sha256 b23b9a4d…)後恢復 PASS(`evidence/20261025-164856/`)。

2026-10-26:把 `download-models.ps1` 的 llama.cpp 壓縮包 SHA256 第一個字元改掉,`verify_download_script.py` 在「A 乾淨目錄下載 exit 0」FAIL、腳本訊息「SHA256 不符,已刪除下載檔」、資料目錄無殘留(證據 `evidence/20261026-003001-dlscript-neg/negative/`);逐位元組還原(sha256 8ae216b1…)。
