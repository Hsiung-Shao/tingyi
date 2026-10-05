✅ 完成於 2026-10-06

# 任務目標

聽譯(tingyi)技術驗證週:在 Tauri 2 + Rust 跑通「系統音訊(WASAPI loopback)→ Silero VAD → SenseVoice-Small(CPU)→ Hy-MT2(llama-server sidecar,GPU)→ 懸浮字幕」完全本機主線,實測端到端延遲(目標句尾後 < 2 秒)、顯示記憶體(< 6 GB)、日文辨識品質與 1.8B / 7B 譯文品質差距,產出驗證報告並回填規格書。

完整計畫:`C:\Users\jerry\.claude\plans\immutable-hatching-journal.md`(已核准 2026-10-05)。

# Task

- [x] 0. 專案骨架:git init、create-tauri-app(React + TS + pnpm)、Cargo workspace(`crates/tingyi-core`、`crates/tingyi-bench`)、`pnpm tauri dev` 開出空視窗
  - 2026-10-06:git init、骨架、workspace、Tailwind 4 完成;core 13 個單元測試通過;前端 `pnpm build` 通過;Tauri 後端編譯中
  - 問題:`pnpm create tauri-app` 會因 dlx 找不到 package.json 失敗,改 `npx create-tauri-app@latest` 可用
  - 問題:Tailwind 4 自動掃描把 data/ 掃進去,build 88 秒 → index.css 改 `source(none)` + 明確 `@source`
- [x] 1. 模型與執行檔下載:llama-server Vulkan b11405、SenseVoice int8 2024-07-17(原抓 2025-09-09,第 3 項發現為粵語微調後換掉)、Silero VAD、Hy-MT2 1.8B Q8 / 1.8B Q4 / 7B Q4 全部到位(7.5 GB)
  - 存放改為專案內 `data/`(gitignore):Claude 桌面版是 MSIX,`%LOCALAPPDATA%` 寫入被重導到 Packages\...\LocalCache,外部程式看不到;paths.rs 改三段 fallback
  - GitHub Releases 當日僅 70–175 KB/s,SenseVoice 改抓 HF 鏡像 csukuangfj(同內容)
  - 問題:兩個下載腳本同時寫同一個 .part 導致 7B 損毀,已砍掉重抓
  - 問題:`taskkill /F` 回報成功但 llama-server 仍活著,改 PowerShell `Process.Kill()` + `Get-Process` 驗證(對應 memory error_taskkill_fails_but_process_alive)
  - **第一組實測**:llama-server 1.8B Q8 ctx 4096 載入 10 秒,顯示記憶體 +2,161 MiB(4742 − 2581);curl 測一句英文/日文:生成 208 tok/s,冷啟動 prompt 1,545 ms、有快取 15 ms;譯文品質為自然台灣口語
- [x] 2. 音訊擷取 + VAD:`audio/{mod,capture}.rs`(重取樣用 sherpa-onnx LinearResampler,未另用 rubato;環形緩衝併入 vad.rs history)、`vad.rs`
  - 2026-10-06 live 實測:Razer 喇叭 48 kHz 2ch → 16 kHz mono 正常;TTS 六句被切成 11 段,段長 1.4–3.4 秒
  - 觀察:第 1 段長 10.3 秒(超過 max_speech 8 s)且開頭含雜音被幻聽成日文假名;待用真實影片再驗
- [x] 3. SenseVoice 辨識:`asr.rs`(Transcriber trait、字集啟發式語言判斷、標籤剝除);`tingyi-bench asr-file` 離線回歸
  - 問題:2025-09-09 版是粵語微調 → 改 2024-07-17 原版(見 memory error_sensevoice_2025_09_09_is_cantonese_finetune)
  - 實測:test_wavs 五語全對,RTF ≈ 0.015;live 11 句辨識 p50 52 ms、max 150 ms
- [x] 4. Hy-MT2 翻譯:`llama.rs`(子程序,取代原訂 tauri sidecar plugin)、`mt.rs`(串流、上下文 6 句、術語表、15 秒逾時)、`postprocess.rs`(先用內建詞表,OpenCC 待 MVP)
  - live 實測(1.8B Q8):翻譯首字 p50 32 ms、完成 p50 98 ms;端到端首字 p50 95 ms、p95 3,553 ms(一次 3.5 秒停頓,疑似 Vulkan 首次遇新 batch 形狀編譯 shader)
  - 問題:`wait_ready_blocking` 用 reqwest::blocking 在 tokio 內 panic → 改 std TcpStream 手寫 /health
  - 品質觀察:build→建築設計、boss→老闆、Chat→聊吧,需情境提示/術語表
  - 30 句三模型對照完成(bench-out/mt-30.jsonl):1.8B Q8 首字 22 ms/完成 128 ms/2,165 MiB;1.8B Q4 22/100/1,426;7B Q4 45/253/5,089;7B 口語明顯較佳,結論寫入 validation-report §4
- [x] 5. Tauri 串接與懸浮字幕:`pipeline.rs` + emit 事件;React 主視窗(控制、紀錄、指標)+ 懸浮字幕視窗
  - 2026-10-06:`TINGYI_AUTOSTART=1` 自動開始 + TTS 六句 + `scripts/shot-windows.ps1` PrintWindow 截圖驗證;主視窗 10 句端到端 p50 95 / p95 114 ms;懸浮雙語字幕正常
  - 調整:懸浮視窗高度 180 → 230(三句時最上句被切)
- [x] 6. 量測與報告:`docs/validation-report.md`(英文 live、三模型對照、顯示記憶體、結論與建議);最小測試基線(core 13 測試 + pnpm build);規格書已回填實測值與待決問題(Claude Docs rev 29);Obsidian 決策紀錄 1 篇 + 資料筆記 3 篇(vault_verify PASS);初始 commit ed79b20
- [x] 7. **真實影片實測(需使用者在場)**:播放英/日影片各約 3 分鐘跑 `tingyi-bench live --out bench-out/live-<lang>.jsonl`;系統靜音時 loopback 是否仍有訊號;結果回填 validation-report §2、§5、§6 與規格書(日文人工抽樣由使用者決定略過)
  - 本機無日文 TTS 語音,無法自動化;驗證週其餘項目已全部完成
  - 2026-10-06 日文動畫 180 秒已量:端到端首字 p50 159 / p95 194 ms;發現切段(8 秒上限)才是體感延遲主因、歌唱段辨識全錯
  - 2026-10-06 英文電影 180 秒已量:端到端首字 p50 128 / p95 233 ms;短碎片("By.")被單獨翻譯,合併規則要改
  - 2026-10-06 靜音測試:英文輪 60–70 秒系統靜音,第 12、13 段仍辨識出對白 → 靜音不影響 loopback(本機裝置)
  - 日文 12 段對白抽樣評分:使用者決定略過

# 相關檔案

| 檔案 | 操作 | 說明 |
|------|------|------|
| Cargo.toml | 新增 | workspace |
| crates/tingyi-core/src/audio/{mod,capture}.rs | 新增 | WASAPI loopback → 16 kHz mono f32(重取樣用 sherpa-onnx LinearResampler) |
| crates/tingyi-core/src/vad.rs | 新增 | Silero VAD 切段規則、前後緩衝(history 兼環形緩衝)、短句判斷 |
| crates/tingyi-core/src/asr.rs | 新增 | Transcriber trait + SenseVoice、字集語言判斷、標籤剝除 |
| crates/tingyi-core/src/mt.rs | 新增 | Translator trait + llama-server SSE 串流、Hy-MT2 提示組裝 |
| crates/tingyi-core/src/postprocess.rs | 新增 | 內建台灣用語詞表 + 術語表(OpenCC 待 MVP) |
| crates/tingyi-core/src/{pipeline,metrics,llama,gpu,paths}.rs | 新增 | 串接與事件、每句時間戳、llama-server 子程序、nvidia-smi 取樣、資料目錄 |
| crates/tingyi-bench/src/main.rs | 新增 | CLI:devices / vad / asr / asr-file / mt / live / serve |
| src-tauri/{tauri.conf.json,capabilities/default.json} | 修改 | 主視窗 + 透明懸浮視窗、權限 |
| src-tauri/src/{lib,commands,state}.rs | 新增/修改 | Tauri 命令、狀態、事件轉發、llama-server 生命週期 |
| src/{App,Overlay}.tsx、src/lib/* | 修改 | React 主視窗與懸浮字幕 |
| scripts/{download-models,tts-smoke,shot-windows}.ps1 | 新增 | 模型下載、TTS 供音測試、視窗截圖 |
| docs/validation-report.md、docs/bench/* | 新增 | 驗證報告、對照句集 |

# 使用套件

- `wasapi` 0.25:WASAPI loopback 擷取
- `sherpa-onnx` 1.13.8:Silero VAD + SenseVoice 推論 + LinearResampler(自動下載預編譯庫)
- `reqwest` 0.13 + `tokio` + `futures-util`:llama-server SSE 串流
- `serde` / `serde_json`:事件與設定;`crossbeam-channel`:音訊 chunk 傳遞;`clap`:bench CLI
- `tauri` 2 + `tauri-plugin-opener`(llama-server 以 std::process 子程序管理,未用 shell plugin)
- 前端:React 19、Vite 8、Tailwind 4、`@tauri-apps/api`

# 驗證方式

- [x] `pnpm tauri dev` 可啟動;`cargo build --workspace` 成功
- [x] llama-server `/health` 回 ok,`nvidia-smi` 記到顯示記憶體(1.8B Q8 +2.2 GB)
- [x] VAD 切段:TTS 六句切成 11 段,靜音不出段(真實影片含音樂待第 7 項)
- [x] `tingyi-bench asr-file` 五語 test_wavs 全對;真實影片 20 句抽樣待第 7 項
- [x] `tingyi-bench mt` 三模型 30 句對照表(bench-out/mt-30.jsonl)
- [x] 懸浮字幕實際顯示譯文;端到端首字 p50 95 ms
- [x] `cargo test --workspace`(13 passed)與 `pnpm build` 通過
- [x] `/handoff-check` 全 PASS(2026-10-06,第 7 項完成後的最終驗收)
