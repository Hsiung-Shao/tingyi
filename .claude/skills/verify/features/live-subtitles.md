# 即時字幕

## Sub-features

- 開始/停止:載入 SenseVoice、VAD(與選用的翻譯 llama-server、Whisper),懸浮字幕隨開始顯示、停止隱藏。
- 辨識紀錄列:每句編號、語言、辨識毫秒、說話者、原文與譯文;講話中灰字預覽。
- 上一輪有紀錄時按開始會先詢問清除或保留。

## How to get to it (user POV)

主視窗左欄「開始」;右側「辨識紀錄」。播放英/日/韓語音後幾秒內出現字幕。

## Driving it with CDP

`scripts/verify_audio_source.py` 的開始、等字幕、停止三步:按鈕文字 `開始`/`停止`,字幕列 `[data-sentence-id]` 的 `[data-role=source]`。
驗證時關閉翻譯(取消勾選「啟用本機翻譯」),只驗擷取與辨識,不啟動 llama-server。

## Gotchas

- 開始到可辨識約 1–3 秒(載模型);第一次 `pnpm tauri dev` 還要編譯。
- 翻譯關閉時字幕列下方的 `…` 是既有顯示,不代表翻譯卡住。
- 第二次按開始若有舊紀錄會跳「清除/保留」對話框,旅程每次都從新啟動的 app 開始以避開它。
