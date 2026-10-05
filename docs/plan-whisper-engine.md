# 任務目標

加入 Whisper large-v3-turbo(whisper.cpp CUDA 版 whisper-server sidecar)作為高準確度辨識引擎選項;即時預覽仍用 SenseVoice。計畫:`C:\Users\jerry\.claude\plans\immutable-hatching-journal.md`(2026-10-06 核准)。

# Task

- [ ] A. `scripts/build-whisper.ps1`(CUDA 13.1 編譯)+ `download-models.ps1 -WithWhisper`;`paths.rs` 加 whisper 路徑
- [ ] B. `llama.rs` 抽通用 `ServerProcess`,新增 `WhisperServerSpec/Process`(就緒判斷:任何 HTTP 回應)
- [ ] C. `asr.rs` `WhisperServer` 實作 `Transcriber`(WAV 編碼、multipart、幻聽黑名單),`ureq` 依賴,單元測試
- [ ] D. `pipeline.rs` `AsrEngine` 設定、`final_asr` / `preview_asr` 分離
- [ ] E. Tauri `start_session.asr_engine`、`AppState.whisper`、`get_info.asr_engines`;前端引擎下拉
- [ ] F. bench `live --asr whisper`、`asr-file --engine whisper`;A/B 寫進報告
- [ ] 驗證:build 產出 exe、curl /inference、cargo test、pnpm build、使用者實測、/handoff-check

# 相關檔案

| 檔案 | 操作 | 說明 |
|------|------|------|
| scripts/build-whisper.ps1 | 新增 | clone + cmake CUDA 編譯 |
| scripts/download-models.ps1 | 修改 | -WithWhisper |
| crates/tingyi-core/src/{paths,llama,asr,pipeline}.rs | 修改 | 路徑、子程序、引擎、管線 |
| crates/tingyi-core/Cargo.toml | 修改 | ureq |
| crates/tingyi-bench/src/main.rs | 修改 | --asr / --engine |
| src-tauri/src/{commands,state}.rs | 修改 | whisper 子程序、引擎選項 |
| src/App.tsx、src/lib/types.ts | 修改 | 引擎下拉 |

# 使用套件

- `ureq` 3:同步 HTTP(worker 為 std thread)
- whisper.cpp b5130(CUDA 13.1 自行編譯)、`ggml-large-v3-turbo-q8_0.bin`(874 MB)

# 驗證方式

- [ ] `data/bin/whisper/whisper-server.exe` 可啟動,curl `/inference` 上傳 ja.wav 回正確日文
- [ ] `cargo test -p tingyi-core`、`pnpm build`
- [ ] `tingyi-bench asr-file --engine whisper` 五語正確、記錄耗時與顯示記憶體
- [ ] A/B(SenseVoice vs Whisper)寫進 validation-report
- [ ] 使用者 app 實測切換引擎;`/handoff-check` 全 PASS
