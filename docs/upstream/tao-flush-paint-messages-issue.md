# 上游 issue 草稿(未發出;使用者決定先不回報)

> 要發的話:到 https://github.com/tauri-apps/tao/issues/new 貼下面英文內容,或先在 #1140 留言附上這份。發出前請使用者確認文字。

---

**Title:** [Windows] `assertion failed: flush_paint_messages(None, ...)` panic when another process calls PrintWindow on the window during IPC (tray-icon not involved)

**Describe the bug**

The app (Tauri 2.12.1, tao 0.37.1, wry 0.57.0, WebView2, Windows 11 26100) occasionally panics on the main thread:

```
thread 'main' panicked at tao-0.37.1\src\platform_impl\windows\event_loop.rs:2295:11:
assertion failed: flush_paint_messages(None, &userdata.event_loop_runner)
```

and exits with code 101. This is the same assert as #1140, but this app does **not** use `tray-icon` (it is not in the dependency graph for the target), so the tray-icon `WM_DEVICECHANGE` explanation from #1140 does not apply here.

**Steps to reproduce (stress, not deterministic)**

1. Release build (`tauri build`), main window open.
2. Loop: another process takes a screenshot of the app's top-level windows with `PrintWindow` (we use a small PowerShell script that calls `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT /* 2 */)` on every visible top-level window of the process, skipping windows whose width or height is 0), while the page concurrently invokes a Tauri command a few times (`window.__TAURI_INTERNALS__.invoke(...)`).
3. In our runs the panic happened after 3–31 rounds of this loop (seen ~6 times on two different builds of the app); one 300-round run did not reproduce it, so it seems timing/environment dependent.

**Analysis (from reading the source, not confirmed with a backtrace)**

`flush_paint_messages` moves the runner to `HandlingRedrawEvents` and then calls `PeekMessageW(..., WM_PAINT, WM_PAINT, PM_REMOVE | PM_QS_PAINT)` for every owned window. `WM_PAINT` is a synthesized queue message, so `PeekMessageW(hwnd, WM_PAINT, WM_PAINT)` on the other windows does not itself dispatch the thread event target's `WM_PAINT`. But `PeekMessageW` also delivers pending *sent* (non-queued) messages, e.g. the `WM_PRINT` from another process's `PrintWindow`. Our guess (no backtrace): the handler of that message (probably WebView2/COM) runs a nested message loop, and meanwhile IPC paths such as the `EXEC_MSG_ID` branch of `execute_in_thread` (which calls `RedrawWindow(RDW_INTERNALPAINT)` on the thread event target) mark it as needing a repaint again, so its `WM_PAINT` is dispatched inside that nested loop. This would also explain why the screenshot and the IPC calls have to happen at the same time. At that point `runner.redrawing()` is already true, `flush_paint_messages(None, ...)` returns `false`, and the `assert!` fires.

The outer `WM_PAINT` handler (either `public_window_callback` or the thread event target itself) calls `redraw_events_cleared()` and `process_control_flow()` when it finishes, so skipping the nested call looks safe.

**Workaround we ship (local patch)**

```diff
-        } else {
-          // This WM_PAINT handler will never be re-entrant because `flush_paint_messages`
-          // doesn't call WM_PAINT for the thread event target (i.e. this window).
-          assert!(flush_paint_messages(None, &userdata.event_loop_runner));
+        } else if flush_paint_messages(None, &userdata.event_loop_runner) {
           userdata.event_loop_runner.redraw_events_cleared();
           process_control_flow(&userdata.event_loop_runner);
+        } else {
+          log::warn!("thread event target WM_PAINT re-entered while redrawing; skipped");
         }
```

This matches the suggestion in #1140 to replace the assert with a log. Happy to open a PR if this approach is acceptable.

**Platform:** Windows 11 Pro 10.0.26100, tao 0.37.1 (also present in 0.37.2 and `dev`).
