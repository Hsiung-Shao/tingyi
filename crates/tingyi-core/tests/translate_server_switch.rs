//! 共用翻譯伺服器的「字幕優先」切換規則:只假造程序啟動邊界。
#![allow(non_snake_case)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tingyi_core::llama::LlamaServerSpec;
use tingyi_core::translate_server::{Holder, Launch, Running, SharedServer};

#[derive(Default)]
struct Log {
    /// 每次 launch 的完整參數(依序)
    launches: Vec<Vec<String>>,
    shutdowns: usize,
}

struct FakeRunning(Arc<Mutex<Log>>);
impl Running for FakeRunning {
    fn shutdown(&mut self) {
        self.0.lock().unwrap().shutdowns += 1;
    }
}

struct FakeLauncher(Arc<Mutex<Log>>);
impl Launch for FakeLauncher {
    fn launch(&mut self, spec: &LlamaServerSpec) -> anyhow::Result<Box<dyn Running>> {
        self.0.lock().unwrap().launches.push(spec.args());
        Ok(Box::new(FakeRunning(self.0.clone())))
    }
}

const SMALL: &str = "Hy-MT2-1.8B-Q8_0";
const BIG: &str = "Hy-MT2-7B-Q4_K_M";
const Q4: &str = "Hy-MT2-1.8B-Q4_K_M";

fn path(name: &str) -> PathBuf {
    PathBuf::from(format!("models/{name}.gguf"))
}

fn server() -> (SharedServer, Arc<Mutex<Log>>) {
    let log = Arc::new(Mutex::new(Log::default()));
    let s = SharedServer::new(
        PathBuf::from("llama-server.exe"),
        51234,
        "key-abc".into(),
        Box::new(FakeLauncher(log.clone())),
    );
    (s, log)
}

fn arg_after(args: &[String], flag: &str) -> String {
    let i = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("參數缺少 {flag}"));
    args[i + 1].clone()
}

/// 第 n 次 launch 用的模型檔名(不含目錄與副檔名)
fn launched_model(log: &Arc<Mutex<Log>>, n: usize) -> String {
    let m = arg_after(&log.lock().unwrap().launches[n], "-m");
    m.replace('\\', "/").rsplit('/').next().unwrap().trim_end_matches(".gguf").to_string()
}

fn launch_count(log: &Arc<Mutex<Log>>) -> usize {
    log.lock().unwrap().launches.len()
}

// U4:字幕要別的模型時暫時切換,字幕停止後切回網頁要的
#[test]
fn U4_字幕換模型時暫時切換停止後切回() {
    let (mut s, log) = server();
    s.acquire(Holder::Web, SMALL, path(SMALL)).unwrap();
    s.acquire(Holder::Subtitles, BIG, path(BIG)).unwrap();
    s.release(Holder::Subtitles);

    assert_eq!(launch_count(&log), 3, "應依序啟動三次");
    assert_eq!(launched_model(&log, 0), SMALL);
    assert_eq!(launched_model(&log, 1), BIG);
    assert_eq!(launched_model(&log, 2), SMALL);
    let first = log.lock().unwrap().launches[0].clone();
    for n in 1..3 {
        let a = log.lock().unwrap().launches[n].clone();
        assert_eq!(arg_after(&a, "--port"), arg_after(&first, "--port"), "port 要固定,擴充才連得上");
        assert_eq!(arg_after(&a, "--api-key"), arg_after(&first, "--api-key"), "api key 要固定");
    }
    assert_eq!(arg_after(&first, "--port"), "51234");
    assert_eq!(arg_after(&first, "--api-key"), "key-abc");
    assert_eq!(log.lock().unwrap().shutdowns, 2, "每次換模型前要先關舊的");
    assert_eq!(s.model(), Some(SMALL));
    assert!(s.holds(Holder::Web), "網頁在字幕期間不應被踢掉");
    assert!(!s.holds(Holder::Subtitles));
    assert!(s.is_running());
}

// U4b:字幕已在跑 7B,網頁後來取得時沿用不重啟,但記下網頁想要的
#[test]
fn U4b_字幕已跑大模型時網頁取得不切換() {
    let (mut s, log) = server();
    s.acquire(Holder::Subtitles, BIG, path(BIG)).unwrap();
    s.acquire(Holder::Web, SMALL, path(SMALL)).unwrap();

    assert_eq!(launch_count(&log), 1, "字幕優先,網頁取得不該重啟");
    assert_eq!(s.model(), Some(BIG));
    assert_eq!(s.wanted(Holder::Web), Some(SMALL));
    assert!(s.holds(Holder::Web));
}

// U5:網頁先放手時字幕仍在用,不多重啟
#[test]
fn U5_網頁先放手時不多重啟() {
    let (mut s, log) = server();
    s.acquire(Holder::Web, SMALL, path(SMALL)).unwrap();
    s.acquire(Holder::Subtitles, BIG, path(BIG)).unwrap();
    let before = launch_count(&log);
    s.release(Holder::Web);

    assert_eq!(launch_count(&log), before, "字幕還在用,網頁放手不該重啟");
    assert_eq!(s.model(), Some(BIG));
    assert!(s.is_running());
}

// U6:只有網頁持有時換網頁模型,立即切換
#[test]
fn U6_只有網頁持有時換模型立即切換() {
    let (mut s, log) = server();
    s.acquire(Holder::Web, SMALL, path(SMALL)).unwrap();
    s.set_wanted(Holder::Web, Q4, path(Q4)).unwrap();

    assert_eq!(launch_count(&log), 2);
    assert_eq!(launched_model(&log, 1), Q4);
    assert_eq!(s.model(), Some(Q4));
    assert_eq!(s.wanted(Holder::Web), Some(Q4));
}

// U7:字幕持有時換網頁模型,等字幕停止才切
#[test]
fn U7_字幕持有中換網頁模型等字幕停止() {
    let (mut s, log) = server();
    s.acquire(Holder::Web, SMALL, path(SMALL)).unwrap();
    s.acquire(Holder::Subtitles, BIG, path(BIG)).unwrap();
    let before = launch_count(&log);
    s.set_wanted(Holder::Web, Q4, path(Q4)).unwrap();

    assert_eq!(launch_count(&log), before, "字幕持有中不該切換");
    assert_eq!(s.model(), Some(BIG));
    assert_eq!(s.wanted(Holder::Web), Some(Q4));

    s.release(Holder::Subtitles);
    assert_eq!(launch_count(&log), before + 1);
    assert_eq!(launched_model(&log, before), Q4);
    assert_eq!(s.model(), Some(Q4));
    assert!(s.holds(Holder::Web));
}
