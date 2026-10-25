# 任務目標

參考 hayamimi 提高辨識正確率(CER)與效能,並新增「指定程式」音訊來源(執行中可即時切換)。計畫:`C:\Users\jerry\.claude\plans\lively-sniffing-shannon.md`(2026-10-25 核准)。

為什麼不能靠刪除達成:指定程式擷取、CER 量測、專用模型路由都是現有程式沒有的能力(原本只有預設輸出裝置 loopback、沒有任何評測工具);擷取迴圈本身是就地改寫(逐包讀取 → 讀到空),沒有新增並行路徑。

# Task

- [x] 0. 擷取穩定化:讀到空、加大緩衝、不連續計數;bench `capture-probe`(54db82a)
- [x] A. 指定程式音訊來源:`audio/source.rs`、capture 熱切換與重綁、Tauri 命令、前端下拉(f60fe86;審查修正 6af0e8e)
- [ ] B. CER 量測:`scripts/fetch-eval.py`、`eval.rs`、bench `asr-eval`(逐句、串流)、基準數字
- [ ] C. 低成本修補:C1 preroll、C2 清理(重複字、取代表)、C3 短稿補救;逐項 A/B
- [ ] D. 專用模型:ReazonSpeech、Parakeet、路由;量完決定預設
- [ ] E. verify skill、實跑 E2E、報告、決策紀錄、審查、/handoff-check

# 相關檔案

| 檔案 | 操作 | 說明 |
|------|------|------|
| crates/tingyi-core/src/audio/{capture,source}.rs | 修改/新增 | 擷取、程式來源 |
| crates/tingyi-core/src/{pipeline,vad,refine,asr,metrics,lib}.rs | 修改 | 熱切換、preroll、路由 |
| crates/tingyi-core/src/{eval,cleanup,route}.rs | 新增 | 評測、清理、路由(短稿補救 A/B 無益已刪除) |
| crates/tingyi-bench/src/main.rs | 修改 | capture-probe、asr-eval |
| src-tauri/src/{commands,state,lib}.rs | 修改 | 來源命令、引擎選項 |
| src/App.tsx、src/lib/types.ts | 修改 | 來源下拉、取代表、引擎 |
| scripts/fetch-eval.py、scripts/download-models.ps1 | 新增/修改 | 評測集、模型 |

# 使用套件

- `unicode-normalization`(MIT/Apache-2.0):評測 NFKC
- 模型:ReazonSpeech k2(Apache-2.0)、Parakeet TDT 0.6B v2(CC-BY-4.0)

# 驗證方式

- [ ] `cargo test --workspace`、`pnpm build`
- [ ] capture-probe 實測數字
- [ ] asr-eval 數字表寫入 `docs/validation-report.md`
- [ ] 指定程式 E2E(tts-smoke + 切換)證據過 evidence gate
- [ ] `/handoff-check` 全 PASS
