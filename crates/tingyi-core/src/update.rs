//! 自動更新的判斷規則(下載與安裝由 tauri-plugin-updater 做,在 src-tauri/src/update.rs)。

/// 測試用:環境變數 `TINGYI_UPDATE_ENDPOINT` 換掉檢查網址;只接受 http(s),空字串或其他協定視為沒設。
pub fn endpoint_override(value: Option<&str>) -> Option<String> {
    let v = value?.trim();
    (v.starts_with("http://") || v.starts_with("https://")).then(|| v.to_string())
}

/// 要不要跳出「有新版」提示:執行中不打擾;使用者按過「略過這版」的版本不再提示。
pub fn should_notify(running: bool, dismissed: Option<&str>, latest: &str) -> bool {
    !running && dismissed != Some(latest)
}
