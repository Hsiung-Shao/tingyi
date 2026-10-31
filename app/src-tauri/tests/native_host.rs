//! native messaging host:訊息框架與 hello 處理的整合測試。
//! 只 mock 外部行為:程序啟動(AppLauncher)與程序存活(ProcessProbe)。
#![allow(non_snake_case)]

use atx_app_lib::native_host::{decode, encode, handle, AppLauncher, FrameError, HostCtx, ProcessProbe, MAX_OUTGOING};
use serde_json::{json, Value};
use std::io::Cursor;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

// ---------- N3~N6:hello 處理 ----------

// N3:程式執行中直接回連線資訊,不喚起程式
#[test]
fn N3_程式執行中直接回連線資訊() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    write_runtime(&file, &runtime_json(4242, 51234));
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![4242]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_millis(500)));

    assert_ready_reply(&r, 51234);
    assert!(launcher.calls().is_empty(), "程式已在跑,不該再喚起");
}

// N4:程式沒開 → 以 --background 喚起一次,等 runtime.json 出現後回 ready
#[test]
fn N4_程式沒開時喚起並等就緒() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    let writers: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let (f2, w2) = (file.clone(), writers.clone());
    // 啟動器被呼叫後約 1 秒才寫出 runtime.json(另一執行緒)
    let launcher = FakeLauncher::new(move || {
        let f3 = f2.clone();
        w2.lock().unwrap().push(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1000));
            write_runtime(&f3, &runtime_json(5151, 52345));
        }));
    });
    let probe = FakeProbe(vec![5151]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_secs(8)));

    for t in writers.lock().unwrap().drain(..) {
        t.join().unwrap();
    }
    let calls = launcher.calls();
    assert_eq!(calls.len(), 1, "只能喚起一次,實際 {calls:?}");
    assert!(calls[0].iter().any(|a| a == "--background"), "喚起參數要含 --background,實際 {:?}", calls[0]);
    assert_ready_reply(&r, 52345);
}

// N5:喚起後 runtime.json 一直沒出現 → 逾時回 app-error,不卡死
#[test]
fn N5_喚起逾時回明確的app_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    let launcher = FakeLauncher::noop();
    let probe = FakeProbe(vec![]);
    let wait = Duration::from_millis(500);

    let t0 = Instant::now();
    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, wait));
    let took = t0.elapsed();

    assert_eq!(r["ok"], false, "逾時應失敗,實際:{r}");
    assert_eq!(r["error"], "app-error");
    assert!(r["message"].as_str().is_some_and(|m| !m.is_empty()), "要有可顯示的說明,實際:{r}");
    assert_eq!(launcher.calls().len(), 1, "仍應嘗試喚起一次");
    assert!(took >= wait / 2, "不該沒等就放棄,實際 {took:?}");
    assert!(took < wait + Duration::from_millis(1500), "不該遠超過 launch_wait,實際 {took:?}");
}

// N6:runtime.json 有 pid 但程序已死(殘留檔)→ 視為沒在跑,要喚起
#[test]
fn N6_pid已死視為沒在跑並喚起() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime.json");
    write_runtime(&file, &runtime_json(4242, 51234)); // 殘留的舊檔,pid 4242 已死
    let f2 = file.clone();
    // 喚起後新的程式寫入新的 runtime(pid 7777、port 53456)
    let launcher = FakeLauncher::new(move || write_runtime(&f2, &runtime_json(7777, 53456)));
    let probe = FakeProbe(vec![7777]);

    let r = handle(&json!({ "type": "hello" }), &ctx(file, &launcher, &probe, Duration::from_secs(3)));

    assert_eq!(launcher.calls().len(), 1, "殘留檔的 pid 已死,必須喚起程式");
    assert_ready_reply(&r, 53456);
}
