// release 版在 Windows 不要額外開主控台視窗(Tauri 範本預設,請勿移除)。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tingyi_lib::run()
}
