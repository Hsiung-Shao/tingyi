//! native messaging host:訊息框架與 hello 處理的整合測試。
//! 只 mock 外部行為:程序啟動(AppLauncher)與程序存活(ProcessProbe)。
#![allow(non_snake_case)]

use serde_json::{json, Value};
use std::io;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tingyi_lib::native_host::{decode, encode, handle, AppLauncher, FrameError, HostCtx, ProcessProbe, MAX_OUTGOING};

// ---------- 假的外部行為 ----------

/// 記錄被呼叫的參數;`on_launch` 可模擬「程式啟動後寫出 runtime.json」
struct FakeLauncher {
    calls: Mutex<Vec<Vec<String>>>,
    on_launch: Box<dyn Fn() + Send + Sync>,
}

impl FakeLauncher {
    fn new(on_launch: impl Fn() + Send + Sync + 'static) -> Self {
        Self { calls: Mutex::new(Vec::new()), on_launch: Box::new(on_launch) }
    }
    fn noop() -> Self {
        Self::new(|| {})
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl AppLauncher for FakeLauncher {
    fn launch(&self, args: &[&str]) -> io::Result<()> {
        self.calls.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
        (self.on_launch)();
        Ok(())
    }
}

/// 只有清單內的 pid 視為存活
struct FakeProbe(Vec<u32>);

impl ProcessProbe for FakeProbe {
    fn alive(&self, pid: u32) -> bool {
        self.0.contains(&pid)
    }
}

// ---------- fixture(直接寫 JSON,不經被測函式) ----------

fn runtime_json(pid: u32, port: u16) -> Value {
    json!({
        "pid": pid,
        "version": "9.9.9",
        "status": "ready",
        "llama": { "port": port, "apiKey": format!("key-{port}"), "model": "hy-mt2" },
        "bridge": { "url": "http://127.0.0.1:47821/mcp", "token": "bridge-token" },
        "message": null
    })
}

fn runtime_with_status(pid: u32, status: &str) -> Value {
    json!({
        "pid": pid,
        "version": "9.9.9",
        "status": status,
        "llama": null,
        "bridge": null,
        "message": null
    })
}

fn bg_args() -> Vec<Vec<String>> {
    vec![vec!["--background".to_string(), "--serve-web".to_string()]]
}

fn write_runtime(path: &Path, v: &Value) {
    std::fs::write(path, serde_json::to_vec(v).unwrap()).unwrap();
}

fn ctx<'a>(file: PathBuf, launcher: &'a FakeLauncher, probe: &'a FakeProbe, launch_wait: Duration) -> HostCtx<'a> {
    HostCtx {
        runtime_file: file,
        launcher,
        probe,
        launch_wait,
        poll: Duration::from_millis(20),
        app_version: "9.9.9".into(),
    }
}

fn assert_ready_reply(r: &Value, port: u16) {
    assert_eq!(r["ok"], true, "應成功,實際:{r}");
    assert_eq!(r["status"], "ready");
    assert_eq!(r["llama"]["baseUrl"], format!("http://127.0.0.1:{port}/v1"));
    assert_eq!(r["llama"]["apiKey"], format!("key-{port}"));
    assert_eq!(r["llama"]["model"], "hy-mt2");
    assert_eq!(r["bridge"]["url"], "http://127.0.0.1:47821/mcp");
    assert_eq!(r["bridge"]["token"], "bridge-token");
    assert_eq!(r["appVersion"], "9.9.9");
    assert_eq!(r["slots"], json!([2, 3]), "ready 時要附上網頁用的 slots,實際:{r}");
}

/// 非 ready 的回覆不該有 slots(沒有欄位或 null)
fn no_slots(r: &Value) -> bool {
    r.get("slots").map_or(true, |v| v.is_null())
}

// ---------- N1 / N2:訊息框架 ----------

// N1:encode 後 decode 內容相同,且前 4 bytes 是 little-endian 的 body 長度
#[test]
fn N1_訊息框架可來回解碼() {
    let msg = json!({ "type": "hello", "text": "繁體中文與 emoji 🙂", "n": [1, 2, 3] });
    let bytes = encode(&msg).expect("一般大小的訊息應可編碼");

    assert!(bytes.len() > 4, "至少要有 4 bytes 長度前綴與內容");
    let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    assert_eq!(len, bytes.len() - 4, "長度前綴必須是 body 的位元組數(UTF-8,不是字元數)");
    let back = decode(&mut Cursor::new(bytes)).expect("解碼不該出錯");
    assert_eq!(back, Some(msg), "來回後內容必須相同");
}

// N2:回覆超過 1 MB 被拒(Chrome 會斷線),恰好 1 MB 仍可送
#[test]
fn N2_超過1MB的回覆被拒() {
    // {"a":"..."} 除了字串內容還有 8 bytes 固定開銷
    let overhead = serde_json::to_vec(&json!({ "a": "" })).unwrap().len();
    let too_big = json!({ "a": "x".repeat(MAX_OUTGOING + 1 - overhead) });
    assert_eq!(serde_json::to_vec(&too_big).unwrap().len(), MAX_OUTGOING + 1);

    let r = encode(&too_big);
    assert!(matches!(r, Err(FrameError::TooLarge(_))), "1 MB+1 應回 TooLarge,實際:{r:?}");
}

// N2(邊界):恰好 1 MB 的訊息要能編碼,不能多拒
#[test]
fn N2_恰好1MB的回覆可編碼() {
    let overhead = serde_json::to_vec(&json!({ "a": "" })).unwrap().len();
    let exact = json!({ "a": "x".repeat(MAX_OUTGOING - overhead) });
    let bytes = encode(&exact).expect("恰好 1 MB 不該被拒");
    assert_eq!(bytes.len(), 4 + MAX_OUTGOING);
}

// ---------- hello 處理 ----------

// T10:已就緒時直接回覆(含 baseUrl、apiKey、slots),不喚起
#[test]
fn T10_已就緒時直接回覆() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    write_runtime(&file, &runtime_json(4242, 51234));
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![4242]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_millis(500)));

    assert_ready_reply(&r, 51234);
    assert!(launcher.calls().is_empty(), "已就緒,不該再喚起");
}

// T8:程式沒開時以 --background --serve-web 喚起一次,等 ready 後回覆(含 slots)
#[test]
fn T8_程式沒開時以背景參數喚起() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    let f2 = file.clone();
    let launcher = FakeLauncher::new(move || write_runtime(&f2, &runtime_json(5151, 52345)));
    let probe = FakeProbe(vec![5151]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_secs(3)));

    assert_eq!(launcher.calls(), bg_args(), "應恰好喚起一次且參數正確");
    assert_ready_reply(&r, 52345);
}

// T8b:殘留檔的 pid 已死視為沒在跑,以背景參數喚起
#[test]
fn T8b_pid已死視為沒在跑並以背景參數喚起() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    write_runtime(&file, &runtime_json(4242, 51234));
    let f2 = file.clone();
    let launcher = FakeLauncher::new(move || write_runtime(&f2, &runtime_json(7777, 53456)));
    let probe = FakeProbe(vec![7777]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_secs(3)));

    assert_eq!(launcher.calls(), bg_args());
    assert_ready_reply(&r, 53456);
}

// T9:程式開著但網頁伺服器未啟動(stopped / error)→ 送背景參數轉交,等到 ready 再回覆
#[test]
fn T9_程式開著但網頁伺服器未啟動() {
    for status in ["stopped", "error"] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("runtime.json");
        write_runtime(&file, &runtime_with_status(6161, status));
        let f2 = file.clone();
        let launcher = FakeLauncher::new(move || write_runtime(&f2, &runtime_json(6161, 54567)));
        let probe = FakeProbe(vec![6161]);

        let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_secs(3)));

        assert_eq!(launcher.calls(), bg_args(), "status {status} 應轉交一次");
        assert_ready_reply(&r, 54567);
    }
}

// T9b:stopped 一直沒起來 → 逾時回當下狀態(ok、stopped、無 slots)
#[test]
fn T9b_stopped一直沒起來逾時回當下狀態() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    write_runtime(&file, &runtime_with_status(6161, "stopped"));
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![6161]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_millis(400)));

    assert_eq!(r["ok"], true, "實際:{r}");
    assert_eq!(r["status"], "stopped");
    assert!(r["llama"].is_null());
    assert!(no_slots(&r), "非 ready 不該有 slots,實際:{r}");
    assert_eq!(launcher.calls().len(), 1);
}

// T11:等待逾時 (a)沒有 runtime.json → app-error;(b)一直 starting → ok、starting、llama null、無 slots
#[test]
fn T11_等待逾時() {
    let wait = Duration::from_millis(400);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![]);
    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, wait));
    assert_eq!(r["ok"], false, "實際:{r}");
    assert_eq!(r["error"], "app-error");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    write_runtime(&file, &runtime_with_status(6262, "starting"));
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![6262]);
    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, wait));
    assert_eq!(r["ok"], true, "實際:{r}");
    assert_eq!(r["status"], "starting");
    assert!(r["llama"].is_null());
    assert!(no_slots(&r), "非 ready 不該有 slots,實際:{r}");
}

// T11b:下載中 / 未下載且程式存活 → 不喚起,直接回當下狀態
#[test]
fn T11b_下載中或未下載不喚起() {
    for status in ["downloading", "not-downloaded"] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("runtime.json");
        write_runtime(&file, &runtime_with_status(6363, status));
        let launcher = FakeLauncher::noop();
        let probe = FakeProbe(vec![6363]);

        let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_millis(400)));

        assert!(launcher.calls().is_empty(), "status {status} 不該喚起,實際 {:?}", launcher.calls());
        assert_eq!(r["ok"], true, "實際:{r}");
        assert_eq!(r["status"], status);
        assert!(no_slots(&r), "非 ready 不該有 slots,實際:{r}");
    }
}

// T12:不認得的請求 → bad-request
#[test]
fn T12_不認得的請求() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![]);

    let r = handle(&json!({ "type": "nope" }), &ctx(file, &launcher, &probe, Duration::from_millis(100)));

    assert_eq!(r["ok"], false);
    assert_eq!(r["error"], "bad-request");
    assert!(launcher.calls().is_empty());
}
