//! 共用翻譯伺服器:即時字幕與瀏覽器擴充共用一個 llama-server。
//!
//! - 誰需要就 `acquire`(字幕在按「開始」時、擴充在第一次 hello 時),都 `release` 才停止。
//! - port 與 api key 在程式啟動時決定一次;換模型重啟也沿用,擴充不必重新取得端點。
//! - slot 分配:0 字幕快路徑、1 回修、2–3 網頁,互不洗掉提示快取。
//! - 啟動與等待就緒交給 `Launch`(正式版用 `llama.rs`,測試換成假的)。

use crate::llama::LlamaServerSpec;
use anyhow::Result;
use std::collections::HashSet;
use std::path::PathBuf;

pub const SUBTITLE_SLOT: u32 = 0;
pub const REFINE_SLOT: u32 = 1;
pub const WEB_SLOTS: [u32; 2] = [2, 3];
/// `-np`:字幕 2 個 + 網頁 2 個。
pub const PARALLEL: u32 = 4;
/// `-c`:每個 slot 3072 token(`-np` 會把 KV 平分)。
pub const CTX_SIZE: u32 = 12_288;

/// 誰在使用伺服器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Holder {
    Subtitles,
    Web,
}

/// 用戶端連線資訊。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// 例如 `http://127.0.0.1:51234`(不含 `/v1`)。
    pub base_url: String,
    pub port: u16,
    pub api_key: String,
}

/// 執行中的伺服器(正式版是 `LlamaServerProcess`)。
pub trait Running: Send {
    fn shutdown(&mut self);
    /// 給效能面板取樣用。
    fn pid(&self) -> Option<u32> {
        None
    }
}

/// 啟動伺服器並等到就緒。
pub trait Launch: Send {
    fn launch(&mut self, spec: &LlamaServerSpec) -> Result<Box<dyn Running>>;
}

/// 共用伺服器的啟動參數。
pub fn shared_spec(exe: PathBuf, model: PathBuf, port: u16, api_key: &str) -> LlamaServerSpec {
    let mut spec = LlamaServerSpec::new(model, port);
    spec.exe = exe;
    spec.parallel = PARALLEL;
    spec.ctx_size = CTX_SIZE;
    spec.api_key = Some(api_key.to_string());
    spec
}

/// 關主視窗時要做什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// 縮到系統匣(網頁翻譯還在用伺服器)。
    HideToTray,
    /// 結束程式(照原本行為)。
    Quit,
}

pub fn close_action(web_held: bool) -> CloseAction {
    if web_held {
        CloseAction::HideToTray
    } else {
        CloseAction::Quit
    }
}

pub struct SharedServer {
    exe: PathBuf,
    port: u16,
    api_key: String,
    launcher: Box<dyn Launch>,
    /// 執行中的(模型名稱, 程序)。
    running: Option<(String, Box<dyn Running>)>,
    holders: HashSet<Holder>,
}

impl SharedServer {
    pub fn new(exe: PathBuf, port: u16, api_key: String, launcher: Box<dyn Launch>) -> Self {
        Self { exe, port, api_key, launcher, running: None, holders: HashSet::new() }
    }

    /// 固定的連線資訊(伺服器沒在跑也能查)。
    pub fn endpoint(&self) -> Endpoint {
        Endpoint { base_url: format!("http://127.0.0.1:{}", self.port), port: self.port, api_key: self.api_key.clone() }
    }

    /// `holder` 要用 `model_name`(檔案 `model_path`)的伺服器;沒在跑就啟動,模型不同就以同一 port/key 重啟。
    pub fn acquire(&mut self, holder: Holder, model_name: &str, model_path: PathBuf) -> Result<Endpoint> {
        if self.model() != Some(model_name) {
            self.stop();
            let spec = shared_spec(self.exe.clone(), model_path, self.port, &self.api_key);
            let proc = self.launcher.launch(&spec)?;
            self.running = Some((model_name.to_string(), proc));
        }
        self.holders.insert(holder);
        Ok(self.endpoint())
    }

    /// `holder` 不再使用;沒有人使用時停止伺服器。
    pub fn release(&mut self, holder: Holder) {
        self.holders.remove(&holder);
        if self.holders.is_empty() {
            self.stop();
        }
    }

    fn stop(&mut self) {
        if let Some((_, mut proc)) = self.running.take() {
            proc.shutdown();
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// 執行中的模型名稱。
    pub fn model(&self) -> Option<&str> {
        self.running.as_ref().map(|(m, _)| m.as_str())
    }

    pub fn holds(&self, holder: Holder) -> bool {
        self.holders.contains(&holder)
    }

    pub fn pid(&self) -> Option<u32> {
        self.running.as_ref().and_then(|(_, p)| p.pid())
    }
}

// ───────────────────────── 正式版的啟動方式 ─────────────────────────

/// 啟動真正的 llama-server:登錄到 Job Object、寫 log、等 `/health` 就緒。
pub struct LlamaLauncher {
    pub job: Option<std::sync::Arc<crate::job::Job>>,
    pub log_file: Option<PathBuf>,
    /// 7B 冷啟動可能要一分鐘以上。
    pub ready_wait: std::time::Duration,
}

impl Running for crate::llama::LlamaServerProcess {
    fn shutdown(&mut self) {
        crate::llama::LlamaServerProcess::shutdown(self);
    }

    fn pid(&self) -> Option<u32> {
        Some(crate::llama::LlamaServerProcess::pid(self))
    }
}

impl Launch for LlamaLauncher {
    fn launch(&mut self, spec: &LlamaServerSpec) -> Result<Box<dyn Running>> {
        let mut spec = spec.clone();
        spec.log_file = self.log_file.clone();
        let mut p = crate::llama::LlamaServerProcess::spawn(spec)?;
        if let Some(job) = &self.job {
            if let Err(e) = job.assign(p.child()) {
                tracing::warn!("llama-server 加入 Job Object 失敗:{e}");
            }
        }
        if let Err(e) = p.wait_ready_blocking(self.ready_wait) {
            p.shutdown();
            return Err(e);
        }
        Ok(Box::new(p))
    }
}

/// 向系統要一個目前沒人用的本機 port。
pub fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

/// 32 bytes 隨機值的 hex 字串(64 字元)。
pub fn new_api_key() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("系統亂數來源");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct State {
        specs: Vec<LlamaServerSpec>,
        shutdowns: usize,
    }

    struct FakeRunning(Arc<Mutex<State>>);
    impl Running for FakeRunning {
        fn shutdown(&mut self) {
            self.0.lock().unwrap().shutdowns += 1;
        }
    }

    struct FakeLaunch(Arc<Mutex<State>>);
    impl Launch for FakeLaunch {
        fn launch(&mut self, spec: &LlamaServerSpec) -> Result<Box<dyn Running>> {
            self.0.lock().unwrap().specs.push(spec.clone());
            Ok(Box::new(FakeRunning(self.0.clone())))
        }
    }

    fn new_server() -> (SharedServer, Arc<Mutex<State>>) {
        let st = Arc::new(Mutex::new(State::default()));
        let srv = SharedServer::new(PathBuf::from("llama-server.exe"), 51234, "k1".into(), Box::new(FakeLaunch(st.clone())));
        (srv, st)
    }

    fn launches(st: &Arc<Mutex<State>>) -> usize {
        st.lock().unwrap().specs.len()
    }

    fn shutdowns(st: &Arc<Mutex<State>>) -> usize {
        st.lock().unwrap().shutdowns
    }

    fn model_1_8b() -> PathBuf {
        PathBuf::from("models/Hy-MT2-1.8B-Q8_0.gguf")
    }

    /// 旗標後面緊接的值。
    fn flag_value(args: &[String], flag: &str) -> Option<String> {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
    }

    // T1b:尚未 acquire 時不啟動;release 不 panic 也不 shutdown
    #[test]
    fn not_started_before_acquire() {
        let (mut srv, st) = new_server();
        assert!(!srv.is_running());
        assert_eq!(launches(&st), 0);
        srv.release(Holder::Web);
        assert_eq!(shutdowns(&st), 0);
        assert!(!srv.is_running());
    }

    // T1:字幕先取得時啟動伺服器,端點固定
    #[test]
    fn subtitles_acquire_launches_server() {
        let (mut srv, st) = new_server();
        let ep = srv.acquire(Holder::Subtitles, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire");
        assert_eq!(launches(&st), 1);
        assert_eq!(ep, srv.endpoint());
        assert_eq!(ep.port, 51234);
        assert_eq!(ep.api_key, "k1");
        assert_eq!(ep.base_url, "http://127.0.0.1:51234");
        assert!(srv.is_running());
        assert_eq!(srv.model(), Some("Hy-MT2-1.8B-Q8_0"));
    }

    // T2:網頁再取得同模型不重啟,兩者皆持有
    #[test]
    fn web_acquire_same_model_does_not_restart() {
        let (mut srv, st) = new_server();
        let ep1 = srv.acquire(Holder::Subtitles, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 1");
        let ep2 = srv.acquire(Holder::Web, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 2");
        assert_eq!(launches(&st), 1);
        assert_eq!(shutdowns(&st), 0);
        assert_eq!(ep1, ep2);
        assert!(srv.holds(Holder::Subtitles));
        assert!(srv.holds(Holder::Web));
    }

    // T3:字幕釋放時網頁仍持有則不停
    #[test]
    fn release_subtitles_keeps_server_while_web_holds() {
        let (mut srv, st) = new_server();
        srv.acquire(Holder::Subtitles, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 1");
        srv.acquire(Holder::Web, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 2");
        srv.release(Holder::Subtitles);
        assert_eq!(shutdowns(&st), 0);
        assert!(srv.is_running());
        assert!(!srv.holds(Holder::Subtitles));
    }

    // T4:全部釋放才停止
    #[test]
    fn release_all_stops_server() {
        let (mut srv, st) = new_server();
        srv.acquire(Holder::Subtitles, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 1");
        srv.acquire(Holder::Web, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 2");
        srv.release(Holder::Subtitles);
        srv.release(Holder::Web);
        assert_eq!(shutdowns(&st), 1);
        assert!(!srv.is_running());
        assert_eq!(srv.model(), None);
    }

    // T5:換模型時先關再以同 port/key 重啟
    #[test]
    fn switching_model_restarts_with_same_port_and_key() {
        let (mut srv, st) = new_server();
        srv.acquire(Holder::Subtitles, "Hy-MT2-1.8B-Q8_0", model_1_8b()).expect("acquire 1");
        let ep = srv
            .acquire(Holder::Subtitles, "Hy-MT2-7B-Q4_K_M", PathBuf::from("models/Hy-MT2-7B-Q4_K_M.gguf"))
            .expect("acquire 2");
        assert_eq!(shutdowns(&st), 1);
        assert_eq!(launches(&st), 2);
        assert_eq!(ep, srv.endpoint());
        let (a1, a2) = {
            let g = st.lock().unwrap();
            if g.specs.len() < 2 {
                panic!("launch 次數不足 2: {}", g.specs.len());
            }
            (g.specs[0].args(), g.specs[1].args())
        };
        assert_eq!(flag_value(&a1, "--port"), flag_value(&a2, "--port"));
        assert_eq!(flag_value(&a1, "--api-key"), flag_value(&a2, "--api-key"));
        assert_eq!(flag_value(&a2, "--port").as_deref(), Some("51234"));
        assert_eq!(flag_value(&a2, "--api-key").as_deref(), Some("k1"));
        let m = flag_value(&a2, "-m").unwrap_or_default();
        assert!(m.contains("Hy-MT2-7B-Q4_K_M"), "第 2 次 -m 應為 7B: {m}");
        assert_eq!(srv.model(), Some("Hy-MT2-7B-Q4_K_M"));
    }

    // T6:啟動參數完整;Hy-MT2 用 --jinja、TranslateGemma 用 --no-jinja
    #[test]
    fn shared_spec_args_are_complete() {
        let hy = model_1_8b();
        let args = shared_spec(PathBuf::from("llama-server.exe"), hy.clone(), 51234, "k1").args();
        assert_eq!(flag_value(&args, "-m"), Some(hy.to_string_lossy().to_string()));
        assert_eq!(flag_value(&args, "-np").as_deref(), Some("4"));
        assert_eq!(flag_value(&args, "-c").as_deref(), Some("12288"));
        assert_eq!(flag_value(&args, "--host").as_deref(), Some("127.0.0.1"));
        assert_eq!(flag_value(&args, "--port").as_deref(), Some("51234"));
        assert_eq!(flag_value(&args, "--api-key").as_deref(), Some("k1"));
        assert!(args.iter().any(|a| a == "--jinja"), "Hy-MT2 應有 --jinja: {args:?}");

        let tg = PathBuf::from("models/translategemma-4b-it-Q4_K_M.gguf");
        let args = shared_spec(PathBuf::from("llama-server.exe"), tg.clone(), 51234, "k1").args();
        assert_eq!(flag_value(&args, "-m"), Some(tg.to_string_lossy().to_string()));
        assert!(args.iter().any(|a| a == "--no-jinja"), "TranslateGemma 應有 --no-jinja: {args:?}");
        assert!(!args.iter().any(|a| a == "--jinja"), "TranslateGemma 不該有 --jinja: {args:?}");
    }

    // T17:網頁翻譯持有伺服器時關主視窗縮到系統匣,否則結束
    #[test]
    fn close_action_depends_on_web_holder() {
        assert_eq!(close_action(true), CloseAction::HideToTray);
        assert_eq!(close_action(false), CloseAction::Quit);
    }
}
