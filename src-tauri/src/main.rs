// release 版在 Windows 不要額外開主控台視窗(Tauri 範本預設,請勿移除)。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Chrome 以 native messaging host 啟動(參數帶擴充來源)時只跑 host 迴圈,不開視窗
    if std::env::args().any(|a| a.starts_with("chrome-extension://")) {
        std::process::exit(tingyi_lib::run_native_host());
    }
    tingyi_lib::run()
}
