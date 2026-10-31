//! bridge 設定檔合併的整合測試。
#![allow(non_snake_case)]

use atx_app_lib::bridge::{merge_config, Providers, BRIDGE_PORT, EXTENSION_ORIGIN};
use serde_json::{json, Value};

const ORIGIN: &str = "chrome-extension://jamiahhkhimbjajeddmncinemlmfkfhn";

fn parse(s: &str) -> Value {
    let r = serde_json::from_str::<Value>(s);
    assert!(r.is_ok(), "merge_config 應輸出合法 JSON,實際:{s:?}");
    r.unwrap()
}

fn origins(v: &Value) -> Vec<&str> {
    v["allowedOrigins"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default()
}

// B1:既有設定 → 保留 token、port 與其他欄位,只改 providers,並確保含本擴充 origin
#[test]
fn B1_合併既有設定時保留token與port並更新providers() {
    let existing = json!({
        "port": 50123,
        "token": "keep-this-token",
        "allowedOrigins": ["https://other.example"],
        "providers": { "claude": false, "codex": true, "gemini": false },
        "openai": [{ "name": "my-endpoint" }],
        "custom": "keep-me"
    })
    .to_string();

    let out = merge_config(Some(&existing), Providers { claude: true, codex: false, gemini: false }).unwrap();
    let v = parse(&out);

    assert_eq!(v["token"], "keep-this-token", "token 變了會讓已配對的擴充失效");
    assert_eq!(v["port"], 50123, "port 變了會讓已配對的擴充連不上");
    assert_eq!(v["providers"], json!({ "claude": true, "codex": false, "gemini": false }), "providers 應等於傳入值");
    assert!(origins(&v).contains(&ORIGIN), "allowedOrigins 必須含本擴充,實際:{:?}", origins(&v));
    assert!(origins(&v).contains(&"https://other.example"), "既有的 origin 不該被丟掉");
    assert_eq!(v["openai"], json!([{ "name": "my-endpoint" }]), "其他欄位要原樣保留");
    assert_eq!(v["custom"], "keep-me", "未知欄位要原樣保留");
    assert_eq!(EXTENSION_ORIGIN, ORIGIN, "常數與規格的擴充 origin 一致");
}

// B1(None):沒有既有設定 → 新建預設值與隨機 token
#[test]
fn B1_無既有設定時建立預設值與非空且隨機的token() {
    let p = Providers::default();
    let a = parse(&merge_config(None, p).unwrap());
    let b = parse(&merge_config(None, p).unwrap());

    assert_eq!(a["port"], BRIDGE_PORT as u64);
    assert_eq!(a["port"], 47821);
    let ta = a["token"].as_str().unwrap_or("");
    assert!(!ta.is_empty(), "新建設定必須有 token");
    assert_ne!(ta, b["token"].as_str().unwrap_or(""), "token 必須隨機,不可固定值");
    assert!(origins(&a).contains(&ORIGIN), "新建設定也要允許本擴充");
    assert_eq!(a["openai"], json!([]), "新建設定 openai 為空陣列");
    assert_eq!(a["providers"], json!({ "claude": false, "codex": true, "gemini": false }));
}

// B1(壞檔):既有內容無法解析 → 視同沒有,新建
#[test]
fn B1_既有內容無法解析時重新建立() {
    let v = parse(&merge_config(Some("{ not json"), Providers::default()).unwrap());
    assert_eq!(v["port"], 47821);
    assert!(!v["token"].as_str().unwrap_or("").is_empty());
}
