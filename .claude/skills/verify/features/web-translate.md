# 網頁翻譯(瀏覽器擴充 + 共用翻譯伺服器)

## Sub-features

- 擴充(`web/extension`)第一次 hello 時,native host(`tingyi.exe chrome-extension://…`,host 名 `com.jerry.tingyi.host`)讀 `runtime.json`;聽譯沒開或網頁還沒取得伺服器時以 `--background --serve-web` 喚起或轉交給執行中的聽譯。
- 字幕與網頁共用一個 llama-server(`-np 4`):slot 0 字幕快路徑、1 回修、2–3 網頁;port 與 api key 在程式執行期間固定。
- 停止字幕不影響網頁翻譯;網頁翻譯使用中關主視窗只縮到系統匣;系統匣可「停止網頁翻譯」。
- 伺服器意外結束或換模型失敗時,監看執行緒把 runtime 改成 Error,擴充下次 hello 會重新喚起。
- 主視窗「翻譯模型」區塊可在 app 內下載 llama.cpp 與 Hy-MT2 1.8B(腳本放的位置也算已下載)。

## How to get to it (user POV)

安裝聽譯後,在 Chrome 載入擴充(之後上架則直接安裝);開任何網頁按浮動球或 Alt+A 翻譯。聽譯沒開時會自動在背景啟動(只出現系統匣圖示)。

## Driving it with CDP

`scripts/verify_web_translate.mjs`:聽譯主視窗走 WebView2 CDP(9339)按鈕(`開始`、`停止`、勾「啟用本機翻譯」、模型下拉);擴充走 Chromium CDP(9341):service worker 讀 `chrome.storage.local['atx:settings']`、對分頁送 `{type:'toggle'}`、fixture 頁數 `font.atx-target`、popup `#appLinkText`。

## Gotchas

- 開發版預設不寫 native host 註冊(避免把安裝版的註冊搶到開發 exe),旅程設 `TINGYI_REGISTER_HOST=1`。
- 單一實例:使用者自己開著聽譯時,喚起會被轉給那一份,旅程一開始就拒絕雙開。
- 在 Claude 環境內,登錄檔與 `%LOCALAPPDATA%` 被 MSIX 重導;Chromium 與聽譯都在同一個重導範圍內所以旅程一致,但不代表真實桌面也會通過(要使用者從檔案總管驗)。
- 強制結束後由擴充喚起的聽譯是背景模式、用預設模型(這台機器可能是 7B);顯示卡被其他程式占用時 7B 會很慢,喚起那一步給 300 秒。
