//! native host 的聽譯輔助:找瀏覽器主程序、把 pipe 事件轉成 native message。
#![allow(non_snake_case)]

use std::io::Cursor;

use serde_json::{json, Value};
use tingyi_core::audio::source::{AudioSource, ProcInfo, ProcTable};
use tingyi_lib::native_host::{decode, find_browser, forward_events};

fn table(procs: &[(u32, Option<u32>, &str, u64)]) -> ProcTable {
    procs
        .iter()
        .map(|&(pid, parent, exe, start_time)| (pid, ProcInfo { pid, parent, exe: exe.to_string(), start_time }))
        .collect()
}

/// host ← cmd ← 瀏覽器子程序 ← 瀏覽器根 ← explorer(父的啟動時間不晚於子)
fn chain(browser_exe: &str) -> ProcTable {
    table(&[
        (50, None, "explorer.exe", 100),
        (40, Some(50), browser_exe, 200),
        (30, Some(40), browser_exe, 210),
        (20, Some(30), "cmd.exe", 220),
        (10, Some(20), "tingyi.exe", 230),
    ])
}

fn app(s: Option<AudioSource>) -> (u32, String, u64) {
    match s {
        Some(AudioSource::App { pid, exe, start_time }) => (pid, exe, start_time),
        other => panic!("應為 App,實得 {other:?}"),
    }
}

// U18:往上找到瀏覽器祖先,再取同執行檔的最上層
#[test]
fn U18_host找到瀏覽器主程序() {
    assert_eq!(app(find_browser(10, &chain("chrome.exe"))), (40, "chrome.exe".to_string(), 200), "要取根 chrome 而非子程序");
    assert_eq!(app(find_browser(10, &chain("msedge.exe"))), (40, "msedge.exe".to_string(), 200));
}

#[test]
fn U18_瀏覽器執行檔名不分大小寫() {
    let (pid, _, _) = app(find_browser(10, &chain("CHROME.EXE")));
    assert_eq!(pid, 40);
}

#[test]
fn U18_沒有瀏覽器祖先回None() {
    let t = table(&[(50, None, "explorer.exe", 100), (20, Some(50), "cmd.exe", 220), (10, Some(20), "tingyi.exe", 230)]);
    assert!(find_browser(10, &t).is_none());
}

#[test]
fn U18_host不在程序表回None() {
    assert!(find_browser(999, &chain("chrome.exe")).is_none());
}

// U19:每行 JSON 轉成一則 native message,壞行與空行略過,結尾補 ended
#[test]
fn U19_host轉送pipe事件() {
    let input = concat!(
        "{\"type\":\"transcript\",\"id\":1,\"text\":\"Hello\"}\n",
        "\n",
        "{\"type\":\"partial\",\"text\":\"wor\"}\n",
        "{這行壞掉\n",
        "{\"type\":\"translation_done\",\"id\":1,\"text\":\"哈囉\"}\n",
    );
    let mut out: Vec<u8> = Vec::new();
    forward_events(Cursor::new(input.as_bytes()), &mut out).unwrap();

    let mut rd = Cursor::new(out);
    let mut got: Vec<Value> = Vec::new();
    while let Some(v) = decode(&mut rd).unwrap() {
        got.push(v);
    }
    assert_eq!(
        got,
        vec![
            json!({"type":"transcript","id":1,"text":"Hello"}),
            json!({"type":"partial","text":"wor"}),
            json!({"type":"translation_done","id":1,"text":"哈囉"}),
            json!({"type":"ended"}),
        ]
    );
}

#[test]
fn U19_pipe直接結束仍送ended() {
    let mut out: Vec<u8> = Vec::new();
    forward_events(Cursor::new(Vec::<u8>::new()), &mut out).unwrap();
    let mut rd = Cursor::new(out);
    assert_eq!(decode(&mut rd).unwrap(), Some(json!({"type":"ended"}))); // 沒送 ended,分頁會一直以為還在聽
    assert_eq!(decode(&mut rd).unwrap(), None);
}
