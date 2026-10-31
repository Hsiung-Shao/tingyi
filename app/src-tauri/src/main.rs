// release 版在 Windows 不要額外開主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Chrome 啟動 native messaging host 時第一個參數是 `chrome-extension://<id>/`;
    // 這時只跑 stdio 迴圈,不初始化 Tauri,也不經過單一實例檢查
    if std::env::args().skip(1).any(|a| a.starts_with("chrome-extension://")) {
        std::process::exit(atx_app_lib::run_native_host());
    }
    atx_app_lib::run(tauri::generate_context!());
}
