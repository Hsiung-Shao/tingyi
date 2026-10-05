# 聽譯(tingyi)技術驗證報告

> 狀態:已完成(2026-10-06)。數字為本機實測,機器:Ryzen 7 9800X3D、RTX 5070 12 GB(Vulkan 1.4)、61 GB RAM、Windows 11 24H2。

## 一句話結論

完全本機的主線(WASAPI → Silero VAD → SenseVoice CPU → Hy-MT2 Vulkan → Tauri 懸浮字幕)已跑通;真實日文動畫與英文電影各 180 秒的端到端首字 p50 分別為 159 ms 與 128 ms、p95 都在 240 ms 內(偶發一次 3.5 秒 Vulkan 熱身離群值),遠低於 2 秒目標;三種翻譯模型顯示記憶體 1.4–5.1 GB 皆在 6 GB 內。真正影響體感的是切段:連續對白常撞到 8 秒上限才出字幕,且歌唱段辨識全錯,這兩項是 MVP 要先處理的。建議 ≥ 8 GB 顯示記憶體預設 7B Q4、否則 1.8B Q8;遊戲術語需靠情境提示與術語表。

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

- 離群值:第 2 句首字 3.5 秒,之後全部回到 20–55 ms。推測是 llama.cpp Vulkan 後端首次遇到新的 batch 形狀時編譯 shader pipeline;熱身只送了一句短句。對策:啟動時用 2–3 種長度熱身(建議 MVP 實測)。
- 固定提示前綴 `cache_prompt` 生效:第二句起 prompt 處理由 1,545 ms 降到 15 ms。
- TTS 句子間有 1.8 秒間隔,比真人講話乾淨;真實影片見下。

第二輪:**日文動畫影片 180 秒**(使用者播放,`bench-out/live-ja.jsonl`),22 段、21 句翻譯,1.8B Q8:

| 階段 | 實測 p50 | 實測 p95 | max |
|---|---|---|---|
| SenseVoice 辨識(段落 1.2–10.4 秒) | 116 ms | 165 ms | 167 ms |
| Hy-MT2 首字(辨識後) | 42 ms | 62 ms | 63 ms |
| Hy-MT2 完成(辨識後) | 126 ms | 228 ms | 245 ms |
| **端到端首字(VAD 句尾起)** | **159 ms** | **194 ms** | 231 ms |
| 端到端完成 | 258 ms | 357 ms | — |

第三輪:**英文電影對白 180 秒**(使用者播放,`bench-out/live-en.jsonl`),28 段、27 句翻譯,1.8B Q8:

| 階段 | 實測 p50 | 實測 p95 | max |
|---|---|---|---|
| SenseVoice 辨識(段落 1.2–14.4 秒) | 84 ms | 186 ms | 187 ms |
| Hy-MT2 首字(辨識後) | 44 ms | 54 ms | 323 ms(第一句) |
| Hy-MT2 完成(辨識後) | 93 ms | 220 ms | — |
| **端到端首字(VAD 句尾起)** | **128 ms** | **233 ms** | 350 ms |
| 端到端完成 | 196 ms | 349 ms | — |

- 日文那輪沒有再出現 3.5 秒離群值;英文這輪第一句首字 323 ms,之後全在 60 ms 內。三輪合計 59 句,端到端首字最差 3.5 秒(一次)、其餘全部 ≤ 350 ms。
- 英文對白辨識大致正確(含口語 "wanna"、"in a sec"),錯誤集中在輕聲、重疊對話與背景音樂段("Tokyo dung"、"simulateimulator"、"You might see me reason these")。
- 段長:28 段中 9 段 ≥ 8 秒,最長 14.4 秒(超過 max_speech 8 秒的設定,sherpa 的上限在低信心語音連續時沒有硬切),同樣指向「切段策略」是下一步重點。
- 短碎片("Mi."、"By."、"Well.")因 SenseVoice 的 ITN 會補句號,目前的短句合併規則(看句末標點)不會攔住它們,被單獨翻成「米」「由於」;規則要改成「≤ 2 個詞的段落一律等待合併或丟棄」。
- **真正的體感延遲來自切段,不是辨識或翻譯**:動畫對白幾乎不停頓,22 段中有 11 段長 8–10 秒(段長 p50 8.2 秒),代表 VAD 的 500 ms 靜音條件很少成立,多數段落是撞到 8 秒上限才切,字幕因此在一句話講完後還要等到段落結束才出現,且一段常含 2–3 句台詞。對策:min_silence 降到 300 ms、max_speech 降到 5 秒,並實作規格書的「即時預覽」(講話中每秒重辨識灰字)。

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
- 真實日文動畫 180 秒(22 段):
  - 對白段落(#1–#4、#14、#16–#22,共 12 段)辨識大致正確,可直接翻;錯誤多為同音字(棋士→應為騎士)、句尾語氣詞,以及一段內多句台詞被連在一起。
  - 主題曲段落(#5–#13,共 9 段)幾乎全錯:SenseVoice 對有伴奏的歌唱輸出「字 與 字 之間 有 空白」的碎片,翻譯跟著胡說;歌曲尾段 #15「偷爱你」還被判成中文而不翻(合計 10 段受歌唱影響)。規格書「音樂標籤略過無人聲段」要落實:SenseVoice 的 `<|BGM|>` 事件標籤在 Rust 包裝被剝掉,需改走 sys 層讀 event 欄位,或用「token 間空白比例」當歌唱偵測。
  - 使用者抽樣評分:略過(使用者決定不做);以上為模型輸出的自評,對白段的正確率估計七成以上、歌唱段趨近零。

## 6. 規格書「待驗證」項目結論

| 項目 | 結論 |
|---|---|
| 系統或播放器靜音時 loopback 是否仍有訊號 | ✅ 英文輪使用者在約 60–70 秒將系統音量靜音,期間第 12 段(61.8 s)與第 13 段(63.5–71.8 s)仍正常辨識出對白 → 本機(Razer Leviathan V2,USB)靜音不影響 loopback。其他驅動仍可能不同,MVP 文件註明 |
| SenseVoice 日文長句與專有名詞 | 已測:動畫對白 12 段大致可用(同音字如 棋士/騎士 會錯、一段含多句);歌唱段 10 段(#5–#13、#15)全錯,需 BGM 偵測略過 |
| 1.8B 與 7B 品質差距 | 7B 口語與語氣明顯較好,速度差 150 ms 感覺不到;術語問題兩者都有 |
| 整體延遲 < 2 秒 | ✅ 真實影片:日文 p50 159 / p95 194 ms,英文 p50 128 / p95 233 ms;TTS p50 95 ms(另一輪單次 3.5 s 熱身離群)。但體感延遲受切段支配,見 §2 |
| 顯示記憶體 < 6 GB | ✅ 1.8B Q4 1.4 GB、1.8B Q8 2.2 GB、7B Q4 5.1 GB |
| 規格書「1.25-bit 1 GB 以下」選項 | 官方 GGUF repo 無 1.25-bit;以 1.8B Q4_K_M(1.4 GB)取代 |

## 6b. Whisper large-v3-turbo 引擎(2026-10-06 加入)

whisper.cpp b5130 以 CUDA 12.8 本機編譯(`scripts/build-whisper.ps1`,Ninja + vcvars64,arch 120),`ggml-large-v3-turbo-q8_0.bin`,whisper-server sidecar(port 8179)。官方 test_wavs 對照:

| 檔案 | SenseVoice-Small(CPU) | Whisper turbo q8(CUDA) |
|---|---|---|
| en.wav 7.2 s | 102 ms;「…50 pieces of **code**」「chieftain」 | 210 ms;「…50 pieces of **gold**」「chief then」✅ |
| ja.wav 7.2 s | 99 ms;「持っていき**ない**」 | 177 ms;「持っていけない」✅,標點自然 |
| ko.wav 4.6 s | 67 ms;無分詞空白 | 126 ms;無分詞空白(Whisper 對韓文也常不分詞) |
| zh / yue | 正確 | 正確(粵語輸出繁體) |

- 顯示記憶體 +1,186–1,214 MiB;載入 0.7–1.4 s;每段 130–210 ms(SenseVoice 60–100 ms)。
- 準確度明顯較好,速度仍遠低於目標;即時預覽維持用 SenseVoice(每秒一次)。
- 陷阱:multipart 送 `detect_language=true` 會讓 whisper-server **只偵測語言、回空文字**;`language=auto` 本身就會自動偵測並辨識。
- 陷阱:CMake 的 Visual Studio 產生器需要 CUDA 的 VS 整合(本機沒裝)→ 改 Ninja + vcvars64;本機 CUDA 13.1 目錄只有 stub,實際 nvcc 在 12.8(支援 sm_120)。
- 使用者以直播影片實測(2026-10-06):錯字明顯減少、翻譯也較準。建議有 NVIDIA 卡且顯示記憶體充足時預設 Whisper。

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
