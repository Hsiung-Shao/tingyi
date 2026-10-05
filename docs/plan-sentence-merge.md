# 任務目標

依使用者實測回饋改善斷句與辨識品質:依語言判斷句子是否說完再決定合併(日文「頑張っ|てくれた」不再被切開翻)、極短碎片不單獨翻、被合併的句子不殘留在列表;段落送辨識前做音量正規化並可錄下段落 wav 做 A/B;預設模型依顯示記憶體自動選。計畫:`C:\Users\jerry\.claude\plans\immutable-hatching-journal.md`(2026-10-06 核准)。

# Task

- [x] A. `sentence.rs`:`Completeness` 與 `completeness(text, lang)`(ja/ko/en 規則 + 極短碎片),單元測試含使用者截圖三句
- [x] B. `pipeline.rs::mt_loop`:依完整度決定等待(Complete 立刻 / Incomplete 2.5 s / Unknown 0.8 s),可連續合併,硬上限 12 s / 80 字;新事件 `TranscriptMerged`
- [x] C. 前端 reducer 處理 `transcript_merged`(移除被吸收句、更新原文)
- [x] E. `audio::normalize_rms` + 測試;worker 送辨識前套用(可關);`record_dir` 存段落 wav;bench `live --record`、`asr-file --normalize`
- [x] F. `get_info` 回傳 `recommended_model`,前端預設採用
- [ ] 驗證:cargo test、pnpm build、使用者以同一影片實測、`live --record` + A/B 寫進報告、/handoff-check
- [x] D. 即時預覽:Segmenter.current_speech + worker 每 1 s 預覽辨識 + Partial 事件 + 前端灰字(使用者回報斷句有改善後實作)

# 相關檔案

| 檔案 | 操作 | 說明 |
|------|------|------|
| crates/tingyi-core/src/sentence.rs | 新增 | 句子完整度判斷 |
| crates/tingyi-core/src/pipeline.rs | 修改 | 合併層、正規化、錄音、新事件 |
| crates/tingyi-core/src/audio/mod.rs | 修改 | normalize_rms |
| crates/tingyi-bench/src/main.rs | 修改 | --record、--normalize |
| src-tauri/src/commands.rs | 修改 | recommended_model |
| src/lib/{types,useSubtitles}.ts、src/App.tsx | 修改 | 新事件、預設模型 |

# 使用套件

既有依賴,無新增。

# 驗證方式

- [x] `cargo test -p tingyi-core` 含 sentence 與 normalize 測試(20 passed)
- [x] `pnpm build`
- [~] 使用者實測:斷句合併已回報改善;即時預覽與音量正規化效果待回報
- [ ] `tingyi-bench live --record` 存段落;`asr-file --normalize` A/B 結果寫進 validation-report
- [ ] `/handoff-check` 全 PASS
