# 計畫:停止時隱藏字幕、匯出辨識紀錄

- 建立日期:2026-10-11
- 狀態:實作完成,待使用者實測
- 起因:打包正式版後實用,使用者要求停止後字幕隱藏、可匯出當前辨識紀錄。
- 使用者決定:匯出格式為純文字對照;用另存新檔對話框;程式開啟時字幕也隱藏,按開始才出現。

## 實作項目

- [x] `tauri.conf.json`:懸浮視窗 `visible: false`。
- [x] `commands.rs`:`set_overlay_visible` 共用函式;`start_session` 成功後顯示、`stop_session` 一開始就隱藏並退出拖曳模式;`set_overlay_interactive` 解鎖時叫出字幕、鎖定且未執行時收起。
- [x] `crates/tingyi-core/src/export.rs`:`format_timestamp`、`to_text`、`write_text_file`(UTF-8 帶 BOM);測試 E1–E9。
- [x] `tauri-plugin-dialog` 與 `@tauri-apps/plugin-dialog`;權限 `dialog:allow-save`;命令 `export_transcript`。
- [x] `App.tsx`:紀錄列上方「匯出紀錄」按鈕(無紀錄時停用)、另存新檔、狀態列回報;監聽 `overlay-interactive` 同步解鎖按鈕。
- [x] `cargo test -p tingyi-core` 56 通過、`cargo check -p tingyi`、`pnpm build`、正式版打包。
- [x] 正式版實跑:啟動後只有主視窗可見(懸浮字幕隱藏),「匯出紀錄」按鈕出現且停用。
- [ ] 使用者實測:開始時字幕出現、停止時消失;停止時解鎖可拖曳、鎖定後消失;匯出檔內容正確、取消不報錯。
- [ ] § 10 review 與 `/handoff-check`。

## 測試規格

| 編號 | 測試標題 | 狀態 |
|---|---|---|
| E1 | 毫秒轉時分秒 | 🟢 |
| E2 | 一般句輸出原文與譯文 | 🟢 |
| E3 | 不翻譯句只有原文 | 🟢 |
| E4 | 失敗與未完成有標示 | 🟢 |
| E5 | 回修與補回有標記 | 🟢 |
| E6 | 說話者與語言標頭 | 🟢 |
| E7 | 缺時間不印時間 | 🟢 |
| E8 | 檔頭含匯出時間與句數 | 🟢 |
| E9 | 寫檔帶 BOM 可讀回 | 🟢 |

## 已知限制

- 按停止時回修一併停止,尚未回修的最後幾句以快譯匯出。
- 時間是相對按下開始的時刻,不是影片時間。
