# 指定程式音訊來源

## Sub-features

- 下拉選「系統全部聲音」,或像 OBS「應用程式音訊擷取」那樣選一個視窗:選項文字 `[exe]: 視窗標題`,分組「播放中」「有視窗的程式」「背景程式(沒有視窗)」。
- 每個可見頂層視窗一項(同程式多視窗各一項,選哪個都擷取整個程式的程序樹,含子程序);程式樹內任一子孫在播放(例如 LINE 的 LineCall)就歸「播放中」。
- 只有音訊 session、沒有視窗的程式列在「背景程式」(同名多個獨立程序時加註 pid)。
- 滑鼠移到下拉或取得焦點時自動重新列舉(2 秒內不重複),也可按「重新整理」。
- 執行中切換:擷取執行緒約 100 ms 內重建,切換點插 700 ms 靜音,回修不跨切換點。
- 目標程式結束:每秒輪詢,依執行檔名改綁新程序;找不到時下拉下方顯示「xxx 已關閉,等它重新開啟…」。

## How to get to it (user POV)

主視窗左欄最上方「音訊來源」。開著視窗的程式都會出現(不必先播放);選視窗,停止或執行中都能改。

## Driving it with CDP

`scripts/verify_audio_source.py`:自己啟動 TTS(`scripts/tts-window.ps1`:powershell.exe 開一個**最小化、不搶前景**的視窗「tingyi verify TTS」並唸英文)與靜音播放器(沒有視窗的 powershell.exe 循環播放靜音 wav),
清單要出現 `[powershell.exe]: tingyi verify TTS`(值 `win:<hwnd>`)且唸的時候在「播放中」,靜音播放器在背景程式(值 `app:<pid>`)
→ 選 TTS 視窗 → 開始 → 應有與 TTS 句子相符的字幕 → 執行中切到靜音播放器 → 12 秒內無新字幕(TTS 仍在唸)→ 切回 TTS 視窗 → 再出現字幕 → 停止。
以 `get_audio_source` 確認後端已切換,並確認下拉顯示的是選的那一項。

## Gotchas

- 第一句常被截掉開頭:TTS 比「開始」早 3 秒啟動,開始時正唸到一半;比對用詞重疊 ≥ 50%,不要求整句。
- 指定程式模式下目標靜音或已結束,process loopback 仍持續送靜音封包(實測),所以「沒字幕」不能證明切換成功,一定要搭配切回後再出現的正向對照。
- 視窗所屬程序與發聲程序不同時,選視窗可能擷取不到:例如 Steam 的視窗屬於 `steamwebhelper.exe`,聲音在它的父程序 `steam.exe`(列在背景程式),要選背景那一項。OBS 也是同樣行為。
- 改綁會綁到同名的任何程序(例如另一個 powershell.exe);重綁的自動化只在單元測試與 `tingyi-bench capture-probe --source app:<pid>` 驗過,GUI 旅程沒有涵蓋。
- 有版權保護的串流(DRM)在 process loopback 可能只有靜音,未測。
