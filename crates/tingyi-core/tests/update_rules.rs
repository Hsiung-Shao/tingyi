use tingyi_core::update::{endpoint_override, should_notify};

// U1:合法 http/https 網址去空白後回傳;None 與空字串回 None
#[test]
fn u1_endpoint_override_接受_http_與_https() {
    assert_eq!(
        endpoint_override(Some("http://127.0.0.1:9/latest.json")),
        Some("http://127.0.0.1:9/latest.json".to_string())
    );
    assert_eq!(
        endpoint_override(Some(" https://x/y ")),
        Some("https://x/y".to_string())
    );
    assert_eq!(endpoint_override(None), None);
    assert_eq!(endpoint_override(Some("")), None);
}

// U2:非 http(s) 協定一律拒絕
#[test]
fn u2_endpoint_override_拒絕其他協定() {
    assert_eq!(endpoint_override(Some("file:///x")), None);
    assert_eq!(endpoint_override(Some("javascript:alert(1)")), None);
    assert_eq!(endpoint_override(Some("ftp://h/x")), None);
}

// U3:辨識中不通知;閒置有新版才通知
#[test]
fn u3_辨識中不彈更新通知() {
    assert!(!should_notify(true, None, "0.1.4"));
    assert!(should_notify(false, None, "0.1.4"));
}

// U4:已略過的版本不再通知,更新的版本照常通知
#[test]
fn u4_略過的版本不再通知_新版照常通知() {
    assert!(!should_notify(false, Some("0.1.3"), "0.1.3"));
    assert!(should_notify(false, Some("0.1.3"), "0.1.4"));
}
