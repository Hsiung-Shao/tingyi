//! native messaging host 設定與登錄機碼內容的整合測試。
#![allow(non_snake_case)]

use atx_app_lib::native_host::HOST_NAME;
use atx_app_lib::register::{host_manifest, registry_entries, EXTENSION_IDS};
use std::path::PathBuf;

// N7:host 設定只允許我們的擴充呼叫
#[test]
fn N7_host設定只允許我們的擴充且路徑為傳入的exe() {
    let exe: PathBuf = std::env::temp_dir().join("atx-app-test").join("atx-app.exe");
    let m = host_manifest(&exe);

    assert_eq!(m["name"], HOST_NAME, "name 必須等於 HOST_NAME,否則擴充找不到 host");
    assert_eq!(m["type"], "stdio");
    assert_eq!(m["path"], exe.to_str().unwrap(), "path 必須是傳入 exe 的絕對路徑");
    assert!(m["description"].as_str().is_some_and(|d| !d.is_empty()), "Chrome 要求 description 非空");
    let expected: Vec<String> = EXTENSION_IDS.iter().map(|id| format!("chrome-extension://{id}/")).collect();
    let got: Vec<String> = m["allowed_origins"]
        .as_array()
        .expect("allowed_origins 必須是陣列")
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(got, expected, "allowed_origins 只能恰好是我們的擴充(結尾要有斜線)");
}

// N8:登錄機碼涵蓋 Chrome、Edge、Chromium 三種瀏覽器
#[test]
fn N8_登錄機碼涵蓋三種瀏覽器且值為manifest路徑() {
    let manifest = std::env::temp_dir().join("atx-app-test").join("native-host.json");
    let mut got = registry_entries(&manifest);
    got.sort();

    let mut expected: Vec<(String, String)> = [
        format!(r"Software\Google\Chrome\NativeMessagingHosts\{HOST_NAME}"),
        format!(r"Software\Microsoft\Edge\NativeMessagingHosts\{HOST_NAME}"),
        format!(r"Software\Chromium\NativeMessagingHosts\{HOST_NAME}"),
    ]
    .into_iter()
    .map(|k| (k, manifest.to_str().unwrap().to_string()))
    .collect();
    expected.sort();

    assert_eq!(got, expected, "三個瀏覽器的機碼都要有,且預設值是 manifest 絕對路徑");
}
