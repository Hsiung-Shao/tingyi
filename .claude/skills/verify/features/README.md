# 聽譯功能地圖

使用者看得到的功能,一個檔一個。改了功能就更新對應檔;地圖列了幾個入口,驗證就要涵蓋幾個。

| 功能 | 檔案 | 自動化旅程 |
|---|---|---|
| 指定程式音訊來源與執行中切換 | [audio-source.md](audio-source.md) | `scripts/verify_audio_source.py` |
| 即時字幕(開始/停止、辨識紀錄) | [live-subtitles.md](live-subtitles.md) | 同上(開始、字幕出現、停止) |
| 專用模型下載(下載、取消、繼續)與「專用模型」引擎 | [routed-models.md](routed-models.md) | `scripts/verify_model_download.py` |
| 開發用模型下載腳本(SHA256 校驗、`-Verify`) | 不另建檔(說明在 `SKILL.md`) | `scripts/verify_download_script.py` |
| 正式版打包(NSIS 安裝檔、release 執行檔) | 不另建檔(說明在 `SKILL.md`) | `scripts/verify_release_build.py` |
| 辨識引擎選項 | 尚未建檔 | `verify_audio_source.py` 檢查下拉選項存在;`verify_model_download.py` 選專用模型並開始 |

尚未建檔:翻譯模型與術語表、辨識取代表、斷句快中慢、說話者改名、懸浮字幕解鎖拖曳、匯出紀錄、效能監測面板。
