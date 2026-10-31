//! native messaging host 設定與登錄機碼內容的整合測試。
#![allow(non_snake_case)]

use std::path::PathBuf;
use tingyi_lib::native_host::HOST_NAME;
use tingyi_lib::register::{host_manifest, registry_entries, should_register, EXTENSION_IDS};

// T13:host 設定只允許我們的擴充,name 為新 host 名稱,path 為傳入的 exe
#[test]
fn T13_host設定內容() {
    assert_eq!(HOST_NAME, "com.jerry.tingyi.host");
    let exe: PathBuf = std::env::temp_dir().join("tingyi-test").join("tingyi.exe");
    let m = host_manifest(&exe);

    assert_eq!(m["name"], "com.jerry.tingyi.host");
    assert_eq!(m["name"], HOST_NAME);
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
    assert!(EXTENSION_IDS.contains(&"jamiahhkhimbjajeddmncinemlmfkfhn"));
    assert_eq!(got, expected, "allowed_origins 只能恰好是我們的擴充(結尾要有斜線)");
}

// T13b:登錄機碼涵蓋 Chrome、Edge、Chromium,結尾是新 host 名稱,值是 manifest 路徑
#[test]
fn T13b_登錄機碼涵蓋三種瀏覽器() {
    let manifest = std::env::temp_dir().join("tingyi-test").join("native-host.json");
    let mut got = registry_entries(&manifest);
    got.sort();

    let mut expected: Vec<(String, String)> = [
        r"Software\Google\Chrome\NativeMessagingHosts\com.jerry.tingyi.host",
        r"Software\Microsoft\Edge\NativeMessagingHosts\com.jerry.tingyi.host",
        r"Software\Chromium\NativeMessagingHosts\com.jerry.tingyi.host",
    ]
    .into_iter()
    .map(|k| (k.to_string(), manifest.to_str().unwrap().to_string()))
    .collect();
    expected.sort();

    assert_eq!(got, expected, "三個瀏覽器的機碼都要有,且預設值是 manifest 絕對路徑");
}

// T14:release 一律註冊;debug 版只有環境變數為 "1" 才註冊
#[test]
fn T14_debug版預設不註冊() {
    assert!(should_register(false, None), "release 版一律註冊");
    assert!(should_register(false, Some("0")), "release 版不看環境變數");
    assert!(!should_register(true, None), "debug 版預設不註冊");
    assert!(should_register(true, Some("1")));
    assert!(!should_register(true, Some("0")));
    assert!(!should_register(true, Some("true")), "只認 1");
}
