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
| 斷句「單段上限」輸入框(`input[aria-label="單段上限(秒)"]`,空白 = 快 3 / 中 8 / 慢 10 秒) | 不另建檔 | `verify_audio_source.py` 第 4 步:執行中填 4 → 標題顯示「上限 4 秒」;清空 → 「上限 8 秒」。切段延遲與準確度另用 `tingyi-bench seg-eval` / `asr-eval --stream` 量 |
| 設定分頁(全部模型下載、「使用這個」選用) | [settings-and-obs.md](settings-and-obs.md) | `scripts/verify_settings_obs.mjs`;引擎下載 `scripts/verify_engine_downloads.mjs`;新電腦基本模型 `scripts/verify_fresh_install.mjs` |
| OBS 直播網址(`/history` 辨識紀錄、`/subtitle` 字幕、`/events`) | [settings-and-obs.md](settings-and-obs.md) | `scripts/verify_settings_obs.mjs` |
| 網頁翻譯(擴充 + 共用翻譯伺服器、native host 喚起、網站規則、網頁模型卡片) | [web-translate.md](web-translate.md) | `scripts/verify_web_translate.mjs` |
| YouTube 沒字幕改用聽譯(播放器內字幕) | [web-translate.md](web-translate.md) | `scripts/verify_youtube_listen.mjs` |
| 懸浮字幕沒聲音自動收起(`input[aria-label="沒聲音幾秒後隱藏字幕"]`,預設 3、0 = 不隱藏) | [live-subtitles.md](live-subtitles.md) | `verify_audio_source.py`:第 1 段懸浮字幕有字、第 2 段切到靜音 10 秒內收起、第 3 段切回只顯示新句、第 5 段設 0 不收/改回 3 收起、第 6 段收起時解鎖拖曳看得到字幕、第 7 段填 100 顯示 30。判斷規則另有 `idle.rs` 單元測試 |

尚未建檔:翻譯模型與術語表、辨識取代表、斷句快中慢按鈕本身、說話者改名、懸浮字幕解鎖拖曳、匯出紀錄、效能監測面板。
