//! 網頁翻譯狀態的純判斷:下載完成後的狀態轉移、與共用伺服器實況比對。
#![allow(non_snake_case)]

use tingyi_lib::runtime::{after_download, web_sync, LlamaStatus, WebSync};

// T23:只有「未下載、網頁沒在用、套件齊了」才改成 Stopped
#[test]
fn T23_下載完成後網頁狀態改為可喚起() {
    use LlamaStatus::*;
    assert_eq!(after_download(NotDownloaded, false, true), Some(Stopped));
    assert_eq!(after_download(NotDownloaded, false, false), None, "套件還沒齊");
    assert_eq!(after_download(NotDownloaded, true, true), None, "網頁正在用");
    assert_eq!(after_download(Ready, true, true), None);
    assert_eq!(after_download(Stopped, false, true), None);
    assert_eq!(after_download(Error, false, true), None);
}

// T25:網頁狀態與伺服器實況比對,每個分支各一例
#[test]
fn T25_網頁狀態與伺服器實況比對() {
    // serving=false → Keep(即使伺服器沒活著)
    assert_eq!(web_sync(false, false, false, None, None), WebSync::Keep);
    assert_eq!(web_sync(false, true, true, Some("7B"), Some("1.8B")), WebSync::Keep);
    // serving=true 但程序已死 → Lost
    assert_eq!(web_sync(true, false, true, None, Some("1.8B")), WebSync::Lost);
    // serving=true 但網頁已不是持有者 → Lost
    assert_eq!(web_sync(true, true, false, Some("1.8B"), Some("1.8B")), WebSync::Lost);
    // 活著、持有、伺服器換了模型 → Model
    assert_eq!(web_sync(true, true, true, Some("7B"), Some("1.8B")), WebSync::Model("7B".to_string()));
    // runtime 沒記模型,伺服器有 → Model
    assert_eq!(web_sync(true, true, true, Some("7B"), None), WebSync::Model("7B".to_string()));
    // 兩者相同 → Keep
    assert_eq!(web_sync(true, true, true, Some("1.8B"), Some("1.8B")), WebSync::Keep);
    // 伺服器沒有模型名稱 → Keep
    assert_eq!(web_sync(true, true, true, None, Some("1.8B")), WebSync::Keep);
}
