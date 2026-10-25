# 指定程式音訊來源

## Sub-features

- 下拉選「系統全部聲音」或某個有音訊 session 的程式(依最上層程序去重;瀏覽器等多程序程式擷取整棵程序樹)。
- 「重新整理」重新列舉;同名多個獨立程序時選項加註 pid;播放中標「播放中」。
- 執行中切換:擷取執行緒約 100 ms 內重建,切換點插 700 ms 靜音,回修不跨切換點。
- 目標程式結束:每秒輪詢,依執行檔名改綁新程序;找不到時下拉下方顯示「xxx 已關閉,等它重新開啟…」。

## How to get to it (user POV)

主視窗左欄最上方「音訊來源」。先讓目標程式播放一下(還沒發過聲的程式不會出現在清單),按「重新整理」,選程式;停止或執行中都能改。

## Driving it with CDP

`scripts/verify_audio_source.py`:自己啟動 TTS(powershell.exe 唸英文)與靜音播放器(powershell.exe 循環播放靜音 wav),
選 TTS → 開始 → 應有與 TTS 句子相符的字幕 → 執行中切到靜音播放器 → 12 秒內無新字幕(TTS 仍在唸)→ 切回 TTS → 再出現字幕 → 停止。
選項值是 `app:<pid>`;以 `get_audio_source` 確認後端已切換。

## Gotchas

- 第一句常被截掉開頭:TTS 比「開始」早 3 秒啟動,開始時正唸到一半;比對用詞重疊 ≥ 50%,不要求整句。
- 指定程式模式下目標靜音或已結束,process loopback 仍持續送靜音封包(實測),所以「沒字幕」不能證明切換成功,一定要搭配切回後再出現的正向對照。
- 改綁會綁到同名的任何程序(例如另一個 powershell.exe);重綁的自動化只在單元測試與 `tingyi-bench capture-probe --source app:<pid>` 驗過,GUI 旅程沒有涵蓋。
- 有版權保護的串流(DRM)在 process loopback 可能只有靜音,未測。
