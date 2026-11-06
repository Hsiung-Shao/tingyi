# 設定分頁與 OBS 直播網址

## Sub-features
- 設定分頁:翻譯模型 llama.cpp + 10 個(`[data-section=translate] [data-model-id]`)、辨識基本模型、6 個辨識引擎各一區(`[data-engine=<id>]`),每列下載/進度/取消/繼續。
- 「使用這個」:翻譯模型 `[data-role=model-use]`(已下載才有)、引擎 `[data-role=engine-use]`;選中顯示 `model-active` / `engine-active`。執行中停用。等同字幕頁的下拉選單,下次「開始」時載入。
- 字幕頁左欄缺件時只有一行 `[data-role=go-settings]` 連結;開始旁 `[data-role=start-missing]` 說「請到『設定』分頁下載」。
- 直播網址:`[data-role=obs-toggle]` 開關(記在主視窗 localStorage `tingyi.obs`,下次開啟自動打開);網址 `[data-role=obs-url-history]`、`[data-role=obs-url-subtitle]`。
  - `/history?n=6&size=26`:最近 n 句原文+譯文;`/subtitle?mode=translation|bilingual&size=40`:同懸浮字幕(最新一句、閒置淡出)。
  - `/events`:SSE,連上先送 `{"type":"reset"}` 與本回合事件(最多 2000 則),之後即時轉發;`{"type":"idle"}` 是閒置。

## How to get to it (user POV)
主視窗標題列旁「設定」分頁。直播:設定頁最下方勾「開啟直播用網址」→ 複製網址 → OBS 來源「+」→「瀏覽器」貼上,填寬高。

## Driving it with CDP
`scripts/verify_settings_obs.mjs`:分頁按鈕 `[data-tab=settings|main]`;下載 `hy-mt2-1.8b-q4`(1.1 GB)後按使用;OBS 頁用無頭 Edge(`--remote-debugging-port=9341`)開,截圖前 `Emulation.setDefaultBackgroundColorOverride` 設深灰底(頁面背景透明)。Host 檢查用原始 socket 送 `Host: evil.com` 應回 403。

## Gotchas
- 埠從 17880 起找空的(到 17899);實際埠看設定頁網址。
- 只綁 127.0.0.1 且只收 Host 為 127.0.0.1/localhost;別台電腦或區網的 OBS 連不到(刻意的)。
- 開發版的頁面資源轉送給 vite(1420);正式版從程式內嵌的 dist 取(`asset_resolver`),兩條路要各驗一次,正式版用 `verify_release_build.py` 之後手動開 `/history`。
- 關掉開關只停止接新連線;已連上的 OBS 頁面要等它自己斷線或重新載入。
