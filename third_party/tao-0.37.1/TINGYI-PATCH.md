# tao 0.37.1 內嵌修補(tingyi)

這是 [tao](https://github.com/tauri-apps/tao) 0.37.1(Apache-2.0,見 `LICENSE`),從 crates.io 原樣複製而來。只改了一處,根目錄 `Cargo.toml` 用 `[patch.crates-io]` 指向這個目錄。

## 改了什麼

`src/platform_impl/windows/event_loop.rs` 裡,thread event target 視窗的 `WM_PAINT` 分支:

```diff
-        } else {
-          // This WM_PAINT handler will never be re-entrant because `flush_paint_messages`
-          // doesn't call WM_PAINT for the thread event target (i.e. this window).
-          assert!(flush_paint_messages(None, &userdata.event_loop_runner));
+        } else if flush_paint_messages(None, &userdata.event_loop_runner) {
           userdata.event_loop_runner.redraw_events_cleared();
           process_control_flow(&userdata.event_loop_runner);
+        } else {
+          // tingyi patch(…)
+          log::warn!("tao: thread event target WM_PAINT re-entered while redrawing; skipped (tingyi patch)");
         }
```

## 為什麼

- **症狀**:聽譯正式版偶發整個程式 panic,訊息是 `assertion failed: flush_paint_messages(None, &userdata.event_loop_runner)`,exit code 101。
- **重現情境**:外部程式對主視窗做 PrintWindow 截圖重繪,同時有 IPC 在跑(驗證腳本 `verify_release_build.py --stress-rounds N`)。
  - 改版前後的版本都會當,約每 3–31 輪一次,所以是既有問題。
  - 2026-10-26 也有一次 300 輪都沒當,觸發還依賴環境條件。
  - OBS 的視窗擷取與部分截圖工具也會用 PrintWindow。
- **機制**(依原始碼推論,沒有拿到 backtrace):
  - `flush_paint_messages` 把狀態設成 `HandlingRedrawEvents`,再用 `PeekMessageW` 清其他視窗的 WM_PAINT。WM_PAINT 是佇列合成訊息,`PeekMessageW(hwnd, WM_PAINT, WM_PAINT)` 不會直接派送 thread event target 的 WM_PAINT。
  - `PeekMessageW` 會順便處理送入的同步訊息(例如外部 PrintWindow 的 WM_PRINT)。該訊息的處理端(推測是 WebView2/COM)又跑了巢狀訊息迴圈。
  - 同時 IPC 等路徑(例如 `execute_in_thread` 的 `EXEC_MSG_ID` 分支呼叫 `RedrawWindow(RDW_INTERNALPAINT)`)把 thread event target 重新標為待重繪,它的 WM_PAINT 就在巢狀迴圈中被派送。這也解釋為何要「截圖 + IPC」同時發生。
  - 這時 `redrawing()` 為真,`flush_paint_messages` 回傳 false,assert 失敗。
- **為什麼跳過是安全的**:
  - 外層的 WM_PAINT(一般視窗的 `public_window_callback`,或 thread event target 自己)離開時,會自行呼叫 `redraw_events_cleared` 與 `process_control_flow` 完成這一輪。
  - 巢狀的那次什麼都不做,不會漏掉狀態轉移,也不會造成 `move_state_to` 的非法轉移。
- **上游**:[tauri-apps/tao#1140](https://github.com/tauri-apps/tao/issues/1140) 是同一個 assert。
  - 當時歸因為 tray-icon 而關閉;聽譯沒有編入 tray-icon。
  - maintainer 在該 issue 提過「把 assert 拿掉改記 log」,但沒有實作。
  - dev 分支與 0.37.2 都仍是 assert。

## 何時移除

- **上游已修**:tao 釋出移除這個 assert 的版本後,刪掉這個目錄與根目錄 `Cargo.toml` 的 `[patch.crates-io]`、`exclude`,改用官方版。
- **tauri 升級後 tao 版本變了**:只要 tauri-runtime-wry 要求的 tao 版本高於 0.37.1(例如 `>=0.37.2` 或 0.38),cargo 就只會警告 `Patch tao v0.37.1 was not used` 並改用 crates.io 帶 assert 的版本。升級 tauri 後要看 cargo 有沒有這個警告;`verify_release_build.py` 的「正式版執行檔含 tao 修補」檢查也會抓到。這時要在新版重做同樣的修改,或確認上游已修。
- **對照原版**:`Cargo.toml.orig`、`Cargo.lock` 等都保留 crates.io 原樣,方便逐檔比對。只有 `event_loop.rs` 與本檔不同。
