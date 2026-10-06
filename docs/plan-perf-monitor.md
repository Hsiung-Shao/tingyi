# 任務目標

效能監測診斷面板:系統與子程序資源(CPU / 記憶體 / GPU / 顯示記憶體)、管線各階段耗時與佇列、端到端趨勢、匯出診斷 JSON。計畫:`C:\Users\jerry\.claude\plans\immutable-hatching-journal.md`(2026-10-06 核准)。

# Task

- [x] A. `metrics.rs` `PipelineStats`(原子計數)+ `pipeline.rs` 各階段更新、handle 暴露
- [x] B. `sysmon.rs` 取樣器(sysinfo + nvidia-smi 一次取齊),環形 5 分鐘
- [x] C. Tauri `get_metrics` / `export_diagnostics`;取樣器只在執行中或面板展開時跑
- [x] D. 前端效能面板(資源卡、管線卡、端到端 SVG 折線、匯出按鈕)
- [x] E. bench live 結束印統計
- [ ] 驗證:cargo test、pnpm build、app 實測數字更新與匯出、/handoff-check

# 相關檔案

| 檔案 | 操作 | 說明 |
|------|------|------|
| crates/tingyi-core/src/sysmon.rs | 新增 | 系統/程序/GPU 取樣 |
| crates/tingyi-core/src/{metrics,gpu,pipeline,lib}.rs | 修改 | 計數與暴露 |
| src-tauri/src/{commands,state}.rs | 修改 | get_metrics、匯出 |
| src/App.tsx、src/lib/types.ts | 修改 | 效能面板 |
| crates/tingyi-bench/src/main.rs | 修改 | 統計 |

# 使用套件

- `sysinfo` 0.37(MIT):程序 CPU / RSS、整機 CPU / 記憶體

# 驗證方式

- [x] `cargo test -p tingyi-core`(28 passed)
- [x] `pnpm build`
- [ ] app:面板每 2 s 更新;子程序啟停時數字出現/歸零;匯出 JSON 成功
- [ ] `/handoff-check` 全 PASS
