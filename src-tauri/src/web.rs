//! 瀏覽器擴充(網頁翻譯)這一側:native host 模式、`runtime.json`、網頁持有共用翻譯伺服器的開關。
//!
//! - Chrome 以 `tingyi.exe chrome-extension://<id>/` 啟動本程式時只跑 native host 迴圈(`run_native_host`)。
//! - 擴充第一次 hello 時,host 以 `--background --serve-web` 喚起或轉交給執行中的程式 → `serve_web`。
//! - 程式把網頁可用的狀態寫進 `runtime.json`,native host 讀它回覆擴充。

use crate::native_host::{self, HostCtx};
use crate::runtime::{self, LlamaInfo, LlamaStatus, Runtime};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tingyi_core::paths;
use tingyi_core::translate_server::Holder;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn runtime_file() -> PathBuf {
    paths::data_dir().join("runtime.json")
}

pub fn settings_file() -> PathBuf {
    paths::data_dir().join("settings.json")
}

/// 網頁翻譯要用的模型:設定的模型沒下載就退回這台機器的預設。
fn web_model() -> String {
    let m = crate::settings::load(&settings_file()).web_model;
    if paths::gguf_model(&m).is_file() {
        m
    } else {
        crate::commands::default_mt_model()
    }
}

pub fn native_host_manifest() -> PathBuf {
    paths::data_dir().join("native-host.json")
}

/// 網頁翻譯的狀態(程式內共用)。
#[derive(Default)]
pub struct WebLink {
    runtime: Mutex<Option<Runtime>>,
    /// 網頁翻譯持有共用伺服器(或正在取得);關主視窗時據此決定縮到系統匣。
    serving: AtomicBool,
}

impl WebLink {
    pub fn serving(&self) -> bool {
        self.serving.load(Ordering::SeqCst)
    }
}

/// 改 runtime 並寫檔,也通知主視窗。
fn update(app: &AppHandle, f: impl FnOnce(&mut Runtime)) {
    let st = app.state::<AppState>();
    let mut g = st.web.runtime.lock().unwrap_or_else(|e| e.into_inner());
    let rt = g.get_or_insert_with(|| Runtime {
        pid: std::process::id(),
        version: VERSION.into(),
        status: LlamaStatus::Stopped,
        llama: None,
        bridge: None,
        message: None,
        listen_pipe: None,
    });
    f(rt);
    if let Err(e) = runtime::write_atomic(&runtime_file(), rt) {
        tracing::warn!("寫 runtime.json 失敗:{e}");
    }
    let _ = app.emit("web-status", rt.clone());
}

fn packages_ready(model: &str) -> bool {
    paths::llama_server_exe().is_file() && paths::gguf_model(model).is_file()
}

/// 記下「YouTube 沒字幕改用聽譯」的 pipe 名稱(native host 從 runtime.json 讀)。
pub fn set_listen_pipe(app: &AppHandle, name: String) {
    update(app, |rt| rt.listen_pipe = Some(name));
}

/// 程式啟動時寫出初始狀態(網頁翻譯尚未啟動)。
pub fn init(app: &AppHandle) {
    let _ = std::fs::create_dir_all(paths::data_dir());
    let model = web_model();
    update(app, |rt| {
        rt.status = if packages_ready(&model) { LlamaStatus::Stopped } else { LlamaStatus::NotDownloaded };
        rt.llama = None;
        rt.message = None;
    });
}

/// 讓網頁翻譯持有共用伺服器(擴充 hello 喚起、主視窗「啟動」)。用網頁自己的模型;字幕正在用別的模型時暫用字幕的。
pub fn serve_web(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.web.serving.swap(true, Ordering::SeqCst) {
        return;
    }
    let slot = st.translate.clone();
    let app = app.clone();
    update(&app, |rt| {
        rt.status = LlamaStatus::Starting;
        rt.llama = None;
        rt.message = None;
    });
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<(u16, String, String)> {
            let mut g = crate::state::lock_translate(&slot)?;
            let s = g.as_mut().expect("lock_translate 已建立");
            let model = web_model();
            if !packages_ready(&model) {
                anyhow::bail!("not-downloaded");
            }
            let ep = s.acquire(Holder::Web, &model, paths::gguf_model(&model))?;
            // 實際跑的可能是字幕的模型
            let running = s.model().unwrap_or(&model).to_string();
            Ok((ep.port, ep.api_key, running))
        })();
        let st = app.state::<AppState>();
        match result {
            Ok((port, api_key, model)) => update(&app, |rt| {
                rt.status = LlamaStatus::Ready;
                rt.llama = Some(LlamaInfo { port, api_key, model });
                rt.message = None;
            }),
            Err(e) => {
                st.web.serving.store(false, Ordering::SeqCst);
                let not_downloaded = e.to_string() == "not-downloaded";
                update(&app, |rt| {
                    rt.llama = None;
                    if not_downloaded {
                        rt.status = LlamaStatus::NotDownloaded;
                        rt.message = Some("翻譯模型尚未下載,請在聽譯主視窗下載".into());
                    } else {
                        rt.status = LlamaStatus::Error;
                        rt.message = Some(format!("本地翻譯模型啟動失敗:{e:#}"));
                    }
                });
            }
        }
    });
}

/// 網頁翻譯不再持有共用伺服器(系統匣「停止網頁翻譯」);字幕還在用就不會停。
pub fn stop_web(app: &AppHandle) {
    let st = app.state::<AppState>();
    if !st.web.serving.swap(false, Ordering::SeqCst) {
        return;
    }
    let slot = st.translate.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(s) = slot.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            s.release(Holder::Web);
        }
        update(&app, |rt| {
            rt.status = LlamaStatus::Stopped;
            rt.llama = None;
            rt.message = None;
        });
    });
}

/// app 內下載完翻譯套件後:原本「未下載」且套件齊了 → 改成 Stopped,擴充下次 hello 就會喚起網頁翻譯。
pub fn refresh_after_download(app: &AppHandle) {
    let st = app.state::<AppState>();
    let status = st.web.runtime.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|rt| rt.status);
    let Some(status) = status else { return };
    let ready = packages_ready(&web_model());
    if let Some(next) = runtime::after_download(status, st.web.serving(), ready) {
        update(app, |rt| {
            rt.status = next;
            rt.message = None;
        });
    }
}

/// 主視窗卡片用的網頁翻譯狀態。
#[derive(serde::Serialize)]
pub struct WebStatus {
    pub status: LlamaStatus,
    pub message: Option<String>,
    pub web_model: String,
    pub server_model: Option<String>,
    pub subtitles: bool,
}

pub fn status(app: &AppHandle) -> WebStatus {
    let st = app.state::<AppState>();
    let rt = st.web.runtime.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // 換模型中會持有鎖:改用 runtime 記的模型,不卡主視窗
    let (server_model, subtitles) = match st.translate.try_lock() {
        Ok(g) => g.as_ref().map(|s| (s.model().map(str::to_string), s.holds(Holder::Subtitles))).unwrap_or((None, false)),
        Err(_) => (rt.as_ref().and_then(|r| r.llama.as_ref().map(|l| l.model.clone())), false),
    };
    WebStatus {
        status: rt.as_ref().map(|r| r.status).unwrap_or(LlamaStatus::Stopped),
        message: rt.and_then(|r| r.message),
        web_model: crate::settings::load(&settings_file()).web_model,
        server_model,
        subtitles,
    }
}

/// 改網頁翻譯的模型並存檔;網頁正在用時依「字幕優先」切換(字幕在用就等它放手)。
pub fn set_web_model(app: &AppHandle, model: String) -> anyhow::Result<()> {
    let mut s = crate::settings::load(&settings_file());
    let previous = std::mem::replace(&mut s.web_model, model.clone());
    crate::settings::save(&settings_file(), &s)?;
    let st = app.state::<AppState>();
    if !st.web.serving() {
        refresh_after_download(app);
        return Ok(());
    }
    let slot = st.translate.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        let r = (|| -> anyhow::Result<Option<String>> {
            let mut g = crate::state::lock_translate(&slot)?;
            let s = g.as_mut().expect("lock_translate 已建立");
            s.set_wanted(Holder::Web, &model, paths::gguf_model(&model))?;
            Ok(s.model().map(str::to_string))
        })();
        match r {
            Ok(m) => update(&app, |rt| {
                if let (Some(l), Some(m)) = (rt.llama.as_mut(), m) {
                    l.model = m;
                }
                rt.message = None;
            }),
            // 伺服器已以原模型恢復;設定也改回去,下次喚起不會再試失敗的模型
            Err(e) => {
                // 等待期間使用者又選了別的模型就不改回,免得蓋掉最新的選擇
                let mut s = crate::settings::load(&settings_file());
                if s.web_model == model {
                    s.web_model = previous;
                    if let Err(e2) = crate::settings::save(&settings_file(), &s) {
                        tracing::warn!("還原網頁翻譯模型設定失敗:{e2}");
                    }
                }
                update(&app, |rt| rt.message = Some(format!("切換網頁翻譯模型失敗,維持原模型:{e:#}")));
            }
        }
    });
    Ok(())
}

/// 每 2 秒比對網頁翻譯狀態與共用伺服器實況:伺服器意外結束 → 寫 Error、放掉網頁持有
/// (擴充下次 hello 會重新喚起);字幕換了模型 → 更新 runtime 的模型名稱。
pub fn start_watch(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        let st = app.state::<AppState>();
        let rt_model = {
            let g = st.web.runtime.lock().unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                // 只在網頁翻譯已就緒時比對,啟動中的狀態由 serve_web 自己寫
                Some(rt) if st.web.serving() && rt.status == LlamaStatus::Ready => rt.llama.as_ref().map(|l| l.model.clone()),
                _ => continue,
            }
        };
        // 啟動或換模型中會持有鎖,這輪先略過
        let Ok(mut g) = st.translate.try_lock() else { continue };
        let Some(s) = g.as_mut() else { continue };
        let alive = s.check_alive();
        let sync = runtime::web_sync(true, alive, s.holds(Holder::Web), s.model(), rt_model.as_deref());
        match sync {
            runtime::WebSync::Keep => {}
            runtime::WebSync::Lost => {
                s.release(Holder::Web);
                drop(g);
                st.web.serving.store(false, Ordering::SeqCst);
                update(&app, |rt| {
                    rt.status = LlamaStatus::Error;
                    rt.llama = None;
                    rt.message = Some("本地翻譯模型已停止,下次翻譯時會重新啟動".into());
                });
            }
            runtime::WebSync::Model(m) => {
                drop(g);
                update(&app, |rt| {
                    if let Some(l) = rt.llama.as_mut() {
                        l.model = m;
                    }
                    rt.message = None;
                });
            }
        }
    });
}

/// 程式結束時刪掉 runtime.json(native host 另外也會檢查 pid)。
pub fn remove_runtime_file() {
    let _ = std::fs::remove_file(runtime_file());
}

// ───────────────────────── native messaging host 模式 ─────────────────────────

struct ExeLauncher;

impl native_host::AppLauncher for ExeLauncher {
    fn launch(&self, args: &[&str]) -> std::io::Result<()> {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let exe = std::env::current_exe()?;
        let spawn = |flags: u32| {
            std::process::Command::new(&exe)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(flags)
                .spawn()
        };
        // 瀏覽器可能把 host 放在 job 裡,host 結束時會連帶殺掉子程序;能脫離就脫離
        spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
            .or_else(|_| spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP))
            .map(|_| ())
    }
}

struct OsProbe;

impl native_host::ProcessProbe for OsProbe {
    fn alive(&self, pid: u32) -> bool {
        tingyi_core::job::pid_alive(pid)
    }
}

/// native messaging host 模式:讀 stdin 的請求、回覆到 stdout,Chrome 關閉管道就結束。
pub fn run_native_host() -> i32 {
    use std::io::Write;
    let ctx = HostCtx {
        runtime_file: runtime_file(),
        launcher: &ExeLauncher,
        probe: &OsProbe,
        // 喚起程式加上載入模型(1.8B 約 3 s、7B 熱啟動約 5 s)
        launch_wait: Duration::from_secs(20),
        poll: Duration::from_millis(200),
        app_version: VERSION.to_string(),
    };
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let req = match native_host::decode(&mut stdin) {
            Ok(Some(v)) => v,
            Ok(None) => return 0,
            Err(_) => return 1,
        };
        // 長連線(connectNative):YouTube 沒字幕改用聽譯,之後這個 host 只做轉送
        if req.get("type").and_then(serde_json::Value::as_str) == Some("listen-start") {
            // 轉送執行緒要寫 stdout:先放掉這裡握著的鎖,否則它會永遠卡住
            drop(stdout);
            return run_listen(stdin, &ctx);
        }
        let reply = native_host::handle(&req, &ctx);
        let Ok(bytes) = native_host::encode(&reply) else { return 1 };
        if stdout.write_all(&bytes).and_then(|_| stdout.flush()).is_err() {
            return 1;
        }
    }
}

/// 送一則訊息給擴充(長連線模式的狀態通知)。
fn send_to_extension(v: &serde_json::Value) {
    use std::io::Write;
    if let Ok(bytes) = native_host::encode(v) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&bytes).and_then(|_| out.flush());
    }
}

/// 長連線模式:找出啟動自己的瀏覽器 → 確認聽譯在跑(沒跑就背景喚起)→ 連 pipe 要求開始;
/// 之後 pipe 的字幕事件轉給擴充。擴充要求停止或斷線(stdin 關閉)就結束,聽譯那邊會停止回合。
fn run_listen(mut stdin: std::io::StdinLock<'_>, ctx: &HostCtx) -> i32 {
    use std::io::Write;
    let fail = |msg: &str| {
        send_to_extension(&serde_json::json!({ "type": "listen_status", "state": "error", "message": msg }));
        1
    };
    let table = tingyi_core::audio::source::snapshot_processes();
    let Some(browser) = native_host::find_browser(std::process::id(), &table) else {
        return fail("找不到啟動擴充的瀏覽器程序");
    };
    send_to_extension(&serde_json::json!({ "type": "listen_status", "state": "starting" }));
    let running = || runtime::read(&ctx.runtime_file).filter(|rt| ctx.probe.alive(rt.pid));
    let mut pipe_name = running().and_then(|rt| rt.listen_pipe);
    if pipe_name.is_none() {
        if let Err(e) = ctx.launcher.launch(&["--background"]) {
            return fail(&format!("無法啟動聽譯:{e}"));
        }
        let t0 = std::time::Instant::now();
        while pipe_name.is_none() && t0.elapsed() < ctx.launch_wait {
            std::thread::sleep(ctx.poll);
            pipe_name = running().and_then(|rt| rt.listen_pipe);
        }
    }
    let Some(name) = pipe_name else { return fail("聽譯沒有回應") };
    // 伺服器正在換下一個 pipe 實例時會短暫忙碌,最多等到與喚起相同的時間
    let mut pipe = None;
    let t0 = std::time::Instant::now();
    while t0.elapsed() < ctx.launch_wait {
        match std::fs::OpenOptions::new().read(true).write(true).open(&name) {
            Ok(f) => {
                pipe = Some(f);
                break;
            }
            Err(_) => std::thread::sleep(ctx.poll),
        }
    }
    let Some(mut pipe) = pipe else { return fail("連不上聽譯") };
    let start = serde_json::json!({ "type": "start", "browser": browser });
    if writeln!(pipe, "{start}").is_err() {
        return fail("連不上聽譯");
    }
    let Ok(reader) = pipe.try_clone() else { return fail("連不上聽譯") };
    // pipe 那邊結束(聽譯停止回合或結束)→ 補一則 ended 後整個 host 結束
    std::thread::spawn(move || {
        let _ = native_host::forward_events(std::io::BufReader::new(reader), &mut std::io::stdout());
        std::process::exit(0);
    });
    // 擴充要求停止或斷線:直接結束,pipe 一關聽譯就停止這個回合
    // (同步 pipe 的讀寫會互相等,轉送執行緒卡在讀取時寫不進停止指令)
    loop {
        match native_host::decode(&mut stdin) {
            Ok(Some(m)) if m.get("type").and_then(serde_json::Value::as_str) != Some("listen-stop") => {}
            _ => return 0,
        }
    }
}
