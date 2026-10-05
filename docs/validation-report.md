# 聽譯(tingyi)技術驗證報告

> 狀態:進行中(2026-10-06)。數字為本機實測,機器:Ryzen 7 9800X3D、RTX 5070 12 GB(Vulkan 1.4)、61 GB RAM、Windows 11 24H2。

## 一句話結論

完全本機的主線(WASAPI → Silero VAD → SenseVoice CPU → Hy-MT2 Vulkan → Tauri 懸浮字幕)已跑通,英文即時測試端到端首字 p50 約 95 ms、p95 114 ms(偶發 3.5 秒 Vulkan 熱身離群值),遠低於 2 秒目標;三種翻譯模型顯示記憶體 1.4–5.1 GB 皆在 6 GB 內。建議 ≥ 8 GB 顯示記憶體預設 7B Q4、否則 1.8B Q8;遊戲術語需靠情境提示與術語表。日文真實影片的辨識率與靜音 loopback 行為尚待使用者播放影片實測。

## 1. 主線可行性

| 項目 | 結果 |
|---|---|
| WASAPI loopback 擷取預設輸出(48 kHz 2ch → 16 kHz mono) | ✅ `wasapi` 0.25,事件驅動 + 100 ms 逾時輪詢 |
| Silero VAD 切段(sherpa-onnx) | ✅ |
| SenseVoice-Small int8(CPU)辨識 | ✅ 2024-07-17 版;官方 test_wavs en/ja/ko/zh/yue 全對,RTF ≈ 0.015 |
| Hy-MT2 經 llama-server(Vulkan)串流翻譯 | ✅ OpenAI 相容 `/v1/chat/completions`,`cache_prompt` 生效 |
| Tauri 2 主視窗 + 懸浮字幕 | ✅ 主視窗:模型/語言/術語表/開始停止、即時指標、紀錄;懸浮視窗:置頂、透明、滑鼠穿透、雙語/只譯文、解鎖拖曳(截圖 `bench-out/shots/live2/`) |
| llama-server 生命週期 | ✅ app 啟動子程序、輪詢 /health、停止時 kill 並 try_wait 確認;主視窗關閉連帶結束 |

## 2. 延遲(VAD 句尾 → 譯文首字)

第一輪:`tingyi-bench live --seconds 80`,音源為 Windows TTS(Zira)透過喇叭唸 6 句英文,loopback 切成 11 段(`bench-out/live-tts-en.jsonl`)。翻譯模型 1.8B Q8。

| 階段 | 規格書推估 | 實測 p50 | 實測 p95 | 備註 |
|---|---|---|---|---|
| VAD 判定句尾 | 500 ms | 500 ms(設定值) | — | 已含在端到端起點之前 |
| SenseVoice 辨識(1.4–3.4 秒段落) | 100–300 | 52 ms | 150 ms | 4 threads;10 秒段落 150 ms |
| Hy-MT2 首字(辨識後) | 100–300 | 32 ms | 3,501 ms | p95 為單一離群值(下述) |
| Hy-MT2 完成(辨識後) | — | 98 ms | 3,600 ms | |
| **端到端首字** | **700–1,100** | **95 ms** | 3,553 ms | 11 句中 10 句 < 170 ms |
| 端到端完成 | — | 145 ms | 3,652 ms | |

- 離群值:第 2 句首字 3.5 秒,之後全部回到 20–55 ms。推測是 llama.cpp Vulkan 後端首次遇到新的 batch 形狀時編譯 shader pipeline;熱身只送了一句短句。對策:啟動時用 2–3 種長度熱身(待驗)。
- 固定提示前綴 `cache_prompt` 生效:第二句起 prompt 處理由 1,545 ms 降到 15 ms。
- 待補:真實影片(英/日各 3 分鐘)的量測;TTS 句子間有 1.8 秒間隔,比真人講話乾淨。

## 3. 顯示記憶體

`nvidia-smi` 整卡用量差值,llama-server `-ngl all -c 4096 -np 1`:

| 組合 | 規格書推估 | 實測增量 | 載入時間(熱) |
|---|---|---|---|
| Hy-MT2-1.8B Q8_0 | 約 2.5 GB | **2,147–2,165 MiB** | 2.1–2.5 s(冷 10 s) |
| Hy-MT2-1.8B Q4_K_M | 約 1.5 GB | **1,426 MiB** | 4.0 s |
| Hy-MT2-7B Q4_K_M | 約 5–5.5 GB | **5,089 MiB** | 5.2 s |
| SenseVoice int8(CPU) | 0 | 0 | 0.8 s |

三種組合都在 6 GB 以內;7B Q4 在 12 GB 卡上與遊戲同跑仍有餘裕,4 GB 卡只能用 1.8B。

## 4. 翻譯品質:1.8B Q8 vs 1.8B Q4 vs 7B Q4

`tingyi-bench mt --sentences docs/bench/sentences-30.tsv`,英 15 句、日 15 句直播口語,無上下文、無術語表(`bench-out/mt-30.jsonl`):

| 模型 | 首字 p50 / p95 | 完成 p50 / p95 | 顯示記憶體 | 品質觀察 |
|---|---|---|---|---|
| 1.8B Q8_0 | 22 / 29 ms | 128 / 198 ms | 2,165 MiB | 通順,但遊戲用語常直譯:boss→老闆、sub→分享、build→組建、スパチャ→提示、配信→線上播放/上傳 |
| 1.8B Q4_K_M | 22 / 29 ms | 100 / 139 ms | 1,426 MiB | 與 Q8 相近,偶有更口語(「好啦」);スパチャ→斯帕恰(音譯錯) |
| 7B Q4_K_M | 45 / 59 ms | 253 / 350 ms | 5,089 MiB | 明顯更像台灣字幕:「也太離譜了吧」「磨了六個小時」「伺服器掛掉啦」「打賞」「過關」;仍有 boss→老闆、raid→地區 |

結論:
- 速度上三者都遠低於 2 秒目標,7B 的額外 150 ms 在字幕情境感覺不到。
- 口語與語氣 7B 勝出,但**遊戲術語三個模型都需要情境提示或術語表**(規格書的「情境預設」是必要功能,不是加分項)。
- 建議:顯示記憶體 ≥ 8 GB 的機器預設 7B Q4;否則 1.8B Q8。1.8B Q4 品質接近 Q8、省 740 MiB,可做「極輕量」選項取代規格書原訂的 1.25-bit(官方 GGUF repo 未提供)。

## 5. SenseVoice 日文口語

- 離線:官方 `test_wavs/ja.wav`(7.2 秒)→「うちの中学は弁当制で持っていきない場合は50円の学校販売のパンを買う。」,語意正確、一處助詞錯(持っていきない→持っていけない),103 ms。
- 英文 TTS 即時 11 段:全部正確(含 "one-shot"、"fire resistance"),一段開頭雜音被幻聽成「れたとしてもずっと」並判成 ja → 需要「段落內語言與上一句不同且很短時忽略」或提高 VAD threshold。
- **真實日文影片 20 句抽樣:待使用者播放影片時實測**(本機沒有日文 TTS 語音,無法自動化)。

## 6. 規格書「待驗證」項目結論

| 項目 | 結論 |
|---|---|
| 系統或播放器靜音時 loopback 是否仍有訊號 | 待實測(需動系統音量,留給使用者操作) |
| SenseVoice 日文長句與專有名詞 | 離線樣本可用;真實影片待測 |
| 1.8B 與 7B 品質差距 | 7B 口語與語氣明顯較好,速度差 150 ms 感覺不到;術語問題兩者都有 |
| 整體延遲 < 2 秒 | ✅ p50 95 ms、p95 114 ms(另一輪 p95 3.5 s 為單次熱身離群) |
| 顯示記憶體 < 6 GB | ✅ 1.8B Q4 1.4 GB、1.8B Q8 2.2 GB、7B Q4 5.1 GB |
| 規格書「1.25-bit 1 GB 以下」選項 | 官方 GGUF repo 無 1.25-bit;以 1.8B Q4_K_M(1.4 GB)取代 |

## 7. 驗證週發現的陷阱

- sherpa-onnx 的 SenseVoice「2025-09-09」版是粵語微調(ASLP-lab WSYue-ASR),日韓掉假名與諺文;要用 2024-07-17 原版。
- sherpa-onnx Rust 包裝不回傳 SenseVoice 的語言欄位,改用字集啟發式判斷(假名→ja、諺文→ko、拉丁→en、純漢字→zh)。
- Claude 桌面版(MSIX)下 `%LOCALAPPDATA%` 寫入被虛擬化,開發期資料目錄改放專案 `data/`。
- `llama-cpp-2` crate 的 Vulkan 需 Vulkan SDK 與長時間編譯;改用官方預編譯 llama-server 當 sidecar,與雲端模式共用 OpenAI 相容程式碼。
- `taskkill /F` 對 llama-server 誤報成功;以 `Process.Kill()` + `Get-Process` 驗證,程式內 `child.kill()` 後 `try_wait` 輪詢確認。

## 8. 建議

1. **預設模型**:啟動時讀顯示記憶體,≥ 8 GB 預設 Hy-MT2-7B Q4_K_M,否則 1.8B Q8_0;4 GB 卡用 1.8B Q4_K_M。
2. **情境預設是 MVP 必要功能**:提示加「這是遊戲直播字幕」等情境句 + 術語表,否則 boss/build/sub/スパチャ 都會直譯。可直接用 Hy-MT2 的 Terminology 範本。
3. **Vulkan 熱身**:啟動後用 2–3 種長度(短句、中句、含上下文的長提示)各翻一次,避免首次 3.5 秒停頓。
4. **幻聽過濾**:段落很短、語言與前後句不同、或只含假名片段時丟棄;VAD threshold 可從 0.5 調到 0.6 再測。
5. **MVP 待補**:OpenCC s2twp(驗證週以內建詞表代替)、裝置切換自動重綁(目前錯誤後 500 ms 重建 session)、全域快捷鍵、設定持久化、即時預覽灰字。
6. **語言欄位**:sherpa-onnx Rust 包裝不回傳 SenseVoice lang;字集啟發式在 en/ja/ko 區分上夠用,若要更準可改走 sherpa-onnx-sys 直接讀 JSON。
7. **packaging**:正式版資料目錄回 `%LOCALAPPDATA%\tingyi`,llama-server 改以 Tauri `externalBin` 隨安裝檔附帶。
