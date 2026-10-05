# 任務目標

斷句自適應(依語言預設 + 依語速自動調 + 快/中/慢三檔)與說話者辨識(聲紋分群、改名、前後文規則)。計畫:`C:\Users\jerry\.claude\plans\immutable-hatching-journal.md`(2026-10-06 核准)。

# Task

- [x] A. `tuner.rs` SegmentationTuner + `SegControl`;`vad.rs` `set_params` 延後重建 VAD;單元測試
- [x] B. `speaker.rs` SpeakerIdentifier(3D-Speaker CAM++ 中英 advanced 模型);改名、分群、前後文規則;單元測試
- [x] C. `pipeline.rs`:tuner 餵資料與套用、說話者欄位與事件、控制面
- [x] D. Tauri 命令 `set_segmentation` / `rename_speaker`;前端斷句區、說話者區、句子標籤、懸浮字幕名字
- [x] E. bench `speakers` 子命令、live 列印;`download-models.ps1 -WithSpeaker`
- [ ] 驗證:cargo test、bench speakers 五語分群、pnpm build、使用者實測、/handoff-check

# 相關檔案

| 檔案 | 操作 | 說明 |
|------|------|------|
| crates/tingyi-core/src/tuner.rs | 新增 | 斷句自適應 |
| crates/tingyi-core/src/speaker.rs | 新增 | 說話者辨識 |
| crates/tingyi-core/src/{vad,pipeline,paths,lib}.rs | 修改 | 參數熱換、事件、路徑 |
| crates/tingyi-bench/src/main.rs | 修改 | speakers 子命令 |
| src-tauri/src/{commands,state}.rs | 修改 | 命令 |
| src/App.tsx、src/Overlay.tsx、src/lib/* | 修改 | UI |
| scripts/download-models.ps1 | 修改 | -WithSpeaker |

# 使用套件

- sherpa-onnx `SpeakerEmbeddingExtractor` / `SpeakerEmbeddingManager`(既有依賴)
- 模型 `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx`(28 MB)

# 驗證方式

- [x] `cargo test -p tingyi-core`(26 passed;tuner 4 測試)
- [x] `tingyi-bench speakers` 五語測試檔分成 5 人,跨檔相似度 ≤ 0.30(門檻 0.55),embedding 70–114 ms
- [x] `pnpm build`
- [ ] 使用者實測:靜音門檻隨語速變動、三檔即時生效;說話者標籤穩定、改名生效
- [ ] `/handoff-check` 全 PASS
