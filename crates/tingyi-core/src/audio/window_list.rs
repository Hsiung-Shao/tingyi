//! 列舉頂層視窗(Win32 `EnumWindows`,依 Z 順序、最上層在前)。只蒐集原始屬性;
//! 過濾與歸屬交給 `source::keep_window` / `window_pid` / `build_source_list`(純函式,有單元測試)。

use super::source::RawWindow;
use std::collections::HashSet;

/// 所有可見的頂層視窗(不可見的不收,反正會被 `keep_window` 濾掉)。
/// `frame_hosts`:ApplicationFrameHost 的 pid;這些視窗額外收集子視窗的 pid(UWP 實際程序)。
#[cfg(windows)]
pub fn enumerate_windows(frame_hosts: &HashSet<u32>) -> Vec<RawWindow> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
    };

    struct Ctx<'a> {
        out: Vec<RawWindow>,
        frame_hosts: &'a HashSet<u32>,
        self_pid: u32,
    }

    fn pid_of(hwnd: HWND) -> u32 {
        let mut pid = 0u32;
        // SAFETY:hwnd 來自 EnumWindows/EnumChildWindows 回呼;輸出指標指向有效的 u32
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        pid
    }

    unsafe extern "system" fn on_child(hwnd: HWND, lp: LPARAM) -> BOOL {
        // SAFETY:lp 是下面 EnumChildWindows 傳入的 &mut Vec<u32>,只在這次呼叫期間使用
        let pids = unsafe { &mut *(lp.0 as *mut Vec<u32>) };
        let pid = pid_of(hwnd);
        if pid != 0 && !pids.contains(&pid) {
            pids.push(pid);
        }
        BOOL(1)
    }

    unsafe extern "system" fn on_window(hwnd: HWND, lp: LPARAM) -> BOOL {
        // SAFETY:lp 是下面 EnumWindows 傳入的 &mut Ctx,只在這次呼叫期間使用
        let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
        // SAFETY:以下都是對回呼給的 hwnd 做唯讀查詢;緩衝區長度由切片帶入
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let pid = pid_of(hwnd);
            // 自己的視窗一律跳過(反正會被排除):對同程序的視窗,GetWindowText 會跨執行緒送 WM_GETTEXT 給
            // UI 執行緒,要等 UI 執行緒處理;先跳過就不會多一條等待 UI 執行緒的路徑。
            // 別的程序的視窗不會送訊息(只讀快取的標題),對方沒回應也不會卡住。
            if pid == ctx.self_pid {
                return BOOL(1);
            }
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut title = vec![0u16; len + 1];
            let n = GetWindowTextW(hwnd, &mut title).max(0) as usize;
            let mut class = [0u16; 256];
            let cn = GetClassNameW(hwnd, &mut class).max(0) as usize;
            let mut cloaked = 0u32;
            let cloaked = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).is_ok()
                && cloaked != 0;
            let mut child_pids = Vec::new();
            if ctx.frame_hosts.contains(&pid) {
                let _ = EnumChildWindows(Some(hwnd), Some(on_child), LPARAM(&mut child_pids as *mut Vec<u32> as isize));
            }
            ctx.out.push(RawWindow {
                hwnd: hwnd.0 as isize,
                pid,
                title: String::from_utf16_lossy(&title[..n.min(len)]),
                class: String::from_utf16_lossy(&class[..cn.min(class.len())]),
                visible: true,
                owned: GetWindow(hwnd, GW_OWNER).is_ok(),
                tool_window: (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW.0 != 0,
                cloaked,
                child_pids,
            });
        }
        BOOL(1)
    }

    let mut ctx = Ctx { out: Vec::new(), frame_hosts, self_pid: std::process::id() };
    // SAFETY:回呼只在 EnumWindows 執行期間被呼叫,ctx 活得比它久
    if let Err(e) = unsafe { EnumWindows(Some(on_window), LPARAM(&mut ctx as *mut Ctx as isize)) } {
        tracing::debug!("EnumWindows 失敗:{e}");
    }
    ctx.out
}

#[cfg(not(windows))]
pub fn enumerate_windows(_frame_hosts: &HashSet<u32>) -> Vec<RawWindow> {
    Vec::new()
}
