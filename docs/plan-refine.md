# 計畫:快路徑先顯示、慢路徑回修(Refine)

- 建立日期:2026-10-06
- 狀態:實作完成,待實測
- 起因:日文直播 5 分鐘錄音對照(報告 §6g)顯示「跳句」除了 Whisper 無時間戳提早收尾(已修)外,還有約 64 秒音訊 VAD 沒切出段落、快語速下等待尾端讓前一句被蓋掉。使用者方向:快路徑先顯示,慢路徑回頭用更長音訊與前後文重做,重點在原文精準。
- 使用者決定:快路徑更積極(未完句靜音 0.6 s 就翻,原 1.2 s);被回修的句子標「修」、補回的標「補」,滑鼠移上去看原快譯。

## 架構

```
vad thread ──RefineMsg::Audio(每 chunk)──┐
asr thread ──RefineMsg::Sentence─────────┼──► refine thread:AudioRing 90 s + Ledger
mt task ────Merged / Translated / Failed─┘      每 250 ms pick_window → Whisper verbose_json
                                                 → align → SourceRevised / Recovered → RefineJob
                                                      └► refine_mt task:等 mt_queue==0,slot 1,帶後文重翻 → TranslationRevised
```

## 實作項目

- [x] `audio/ring.rs`(環形緩衝、全域樣本時鐘)、`audio/wav_sink.rs`(連續 wav)。
- [x] `Utterance` / `Transcript` / `TranscriptMerged` 帶 `start/end`;`merge()` 取區間聯集;`record_all`。
- [x] `incomplete_silence` 1200 → 600 ms。
- [x] `asr.rs`:`transcribe_timestamped`(verbose_json、token_timestamps 關、no_language_probabilities、suppress_non_speech、prompt)、`is_hallucination` 抽出共用、verbose 解析測試。
- [x] `refine.rs`:`RefineConfig`、`RefineMsg`、`RefineJob`、`Ledger`、`normalize_for_compare`、`align`、`pick_window`、`refine_loop`、`refine_mt_loop`;10 個單元測試。
- [x] `pipeline.rs` 接線:三條線送訊息、`refine_thread` / `refine_mt_task`、停止順序、SenseVoice 時發 Status 停用。
- [x] `mt.rs`:`lookahead`、`slot`、`build_prompt` 後文區塊(快路徑位元組不變)、`id_slot`;`llama.rs::with_refine()`(-np 2 / ctx 6144)。
- [x] `metrics.rs`:refine 計數、`mt_queue()`。
- [x] Tauri `StartOptions.refine`;bench `--record-all / --no-refine / --refine-server`、事件列印、結束統計。
- [x] 前端:三個事件 reducer、`Sentence.revised/recovered/prevSource/prevTranslation`、修/補標記(懸停看原文)、回修 checkbox(僅 Whisper)、PerfPanel 回修卡、Overlay 忽略補回句。
- [x] `cargo test -p tingyi-core` 46 通過、`cargo check -p tingyi`、`pnpm build` 通過。
- [ ] 實測:同一支日文影片同起點各錄 5 分鐘(基線 `--no-refine` vs 回修 `--record-all`),量補回句數/字數、原文修正句數與抽樣品質、快路徑 asr p95 / e2e p95 不劣化 ≤ 10%、`mt_first_avg_ms` 不退、VRAM 增量。
- [ ] app 手動驗證:修/補標記與懸停、Overlay 當前句被修會更新、補回句位置、停止再開始不殘留、SenseVoice 時 checkbox 停用。
- [ ] 結果寫 `docs/validation-report.md §6h`;Obsidian 決策紀錄;`/handoff-check`。

## 風險與對策

- 長視窗幻聽 → `suppress_non_speech` + 黑名單 + 補回三道門檻(≥ 300 ms、CJK ≥ 2 字、不與鄰句重複)。
- 時間戳偏移 → 中點判定 + 300 ms 容忍,只在正規化後不同才發。
- whisper-server 全域 mutex → 視窗上限 25 s、關 token_timestamps;asr p95 超標就改 `--refine-server` 第二伺服器。
- 補回句插入觸發列表捲到底 → 先接受。
