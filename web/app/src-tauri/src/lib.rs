pub mod bridge;
pub mod download;
pub mod job;
pub mod llama;
pub mod native_host;
pub mod paths;
pub mod register;
pub mod runtime;

use download::{DownloadOptions, DownloadRegistry, Package};
use job::Job;
use llama::{LlamaProcess, LlamaSpec};
use runtime::{BridgeInfo, LlamaInfo, LlamaStatus, Runtime};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 模型載入最久等多久(第一次載入、Vulkan shader 編譯會比較慢)
const LLAMA_READY_WAIT: Duration = Duration::from_secs(180);

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
        job::pid_alive(pid)
    }
}

/// native messaging host 模式:讀 stdin 的請求、回覆到 stdout,Chrome 關閉管道就結束
pub fn run_native_host() -> i32 {
    use std::io::Write;
    let data = paths::data_dir();
    let ctx = native_host::HostCtx {
        runtime_file: paths::runtime_file(&data),
        launcher: &ExeLauncher,
        probe: &OsProbe,
        launch_wait: Duration::from_secs(20),
        poll: Duration::from_millis(200),
        app_version: VERSION.to_string(),
    };
    let origin = std::env::args().find(|a| a.starts_with("chrome-extension://")).unwrap_or_default();
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let req = match native_host::decode(&mut stdin) {
            Ok(Some(v)) => v,
            Ok(None) => return 0,
            Err(_) => return 1,
        };
        let _ = std::fs::write(
            paths::hello_file(&data),
            serde_json::json!({ "time": now_secs(), "origin": origin }).to_string(),
        );
        let reply = native_host::handle(&req, &ctx);
        let Ok(bytes) = native_host::encode(&reply) else { return 1 };
        if stdout.write_all(&bytes).and_then(|_| stdout.flush()).is_err() {
            return 1;
        }
    }
}

// ───────────────────────── 桌面程式模式 ─────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AppSettings {
    bridge_enabled: bool,
}

struct AppState {
    data: PathBuf,
    bridge_script: PathBuf,
    job: Job,
    runtime: Mutex<Runtime>,
    llama: Mutex<Option<LlamaProcess>>,
    bridge: Mutex<Option<bridge::BridgeProcess>>,
    downloads: Arc<DownloadRegistry>,
    settings: Mutex<AppSettings>,
}

/// 給介面的完整狀態
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusView {
    runtime: Runtime,
    llama_downloaded: bool,
    model_downloaded: bool,
    /// 下載中:(已下載, 總量)
    download: Option<(u64, u64)>,
    browsers: Vec<(String, bool)>,
    /// 擴充最近一次連線(unix 秒)
    last_hello: Option<u64>,
    node: Option<String>,
    bridge_enabled: bool,
    data_dir: String,
}

impl AppState {
    fn packages_ready(&self) -> (bool, bool) {
        (
            download::package_ready(&download::llama_package(), &self.data.join("bin")),
            download::package_ready(&download::model_package(), &paths::models_dir(&self.data)),
        )
    }

    fn model_path(&self) -> PathBuf {
        paths::models_dir(&self.data).join(download::model_package().dir_name).join(download::MODEL_FILE)
    }

    /// 改 runtime、寫檔、通知介面
    fn update(&self, app: &AppHandle, f: impl FnOnce(&mut Runtime)) {
        let rt = {
            let mut rt = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut rt);
            rt.clone()
        };
        if let Err(e) = runtime::write_atomic(&paths::runtime_file(&self.data), &rt) {
            tracing::warn!("寫 runtime.json 失敗:{e}");
        }
        let _ = app.emit("status", ());
    }

    fn view(&self) -> StatusView {
        let (llama_downloaded, model_downloaded) = self.packages_ready();
        let download = ["llama", "model"].iter().find_map(|id| self.downloads.snapshot(id));
        let manifest = paths::native_host_manifest(&self.data);
        let last_hello = std::fs::read_to_string(paths::hello_file(&self.data))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("time")?.as_u64());
        StatusView {
            runtime: self.runtime.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            llama_downloaded,
            model_downloaded,
            download,
            browsers: register::registered_browsers(&manifest).into_iter().map(|(n, ok)| (n.to_string(), ok)).collect(),
            last_hello,
            node: bridge::find_node().map(|p| p.display().to_string()),
            bridge_enabled: self.settings.lock().unwrap_or_else(|e| e.into_inner()).bridge_enabled,
            data_dir: self.data.display().to_string(),
        }
    }

    fn save_settings(&self) {
        let s = self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = std::fs::write(paths::app_settings_file(&self.data), serde_json::to_string_pretty(&s).unwrap_or_default());
    }
}

fn state(app: &AppHandle) -> tauri::State<'_, Arc<AppState>> {
    app.state::<Arc<AppState>>()
}

/// 背景執行緒啟動 llama-server 並等就緒
fn start_model(app: &AppHandle) {
    let st = state(app).inner().clone();
    if st.llama.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return;
    }
    let (llama_ok, model_ok) = st.packages_ready();
    if !(llama_ok && model_ok) {
        st.update(app, |rt| {
            rt.status = LlamaStatus::NotDownloaded;
            rt.llama = None;
        });
        return;
    }
    st.update(app, |rt| {
        rt.status = LlamaStatus::Starting;
        rt.llama = None;
        rt.message = None;
    });
    let app = app.clone();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<LlamaInfo> {
            let port = llama::free_port()?;
            let _ = std::fs::create_dir_all(paths::logs_dir(&st.data));
            let mut spec = LlamaSpec::new(paths::llama_server_exe(&st.data), st.model_path(), port);
            spec.log_file = Some(paths::logs_dir(&st.data).join("llama-server.log"));
            let info = LlamaInfo { port, api_key: spec.api_key.clone(), model: llama::MODEL_NAME.into() };
            let mut proc = LlamaProcess::spawn(spec, &st.job)?;
            proc.wait_ready_blocking(LLAMA_READY_WAIT)?;
            *st.llama.lock().unwrap_or_else(|e| e.into_inner()) = Some(proc);
            Ok(info)
        })();
        match result {
            Ok(info) => st.update(&app, |rt| {
                rt.status = LlamaStatus::Ready;
                rt.llama = Some(info);
                rt.message = None;
            }),
            Err(e) => st.update(&app, |rt| {
                rt.status = LlamaStatus::Error;
                rt.llama = None;
                rt.message = Some(format!("本地模型啟動失敗:{e}"));
            }),
        }
    });
}

fn stop_model(app: &AppHandle) {
    let st = state(app).inner().clone();
    if let Some(mut p) = st.llama.lock().unwrap_or_else(|e| e.into_inner()).take() {
        p.shutdown();
    }
    st.update(app, |rt| {
        rt.status = LlamaStatus::Stopped;
        rt.llama = None;
        rt.message = None;
    });
}

/// 依設定啟動或停止 bridge
fn apply_bridge(app: &AppHandle) {
    let st = state(app).inner().clone();
    let enabled = st.settings.lock().unwrap_or_else(|e| e.into_inner()).bridge_enabled;
    if let Some(mut b) = st.bridge.lock().unwrap_or_else(|e| e.into_inner()).take() {
        b.shutdown();
    }
    if !enabled {
        st.update(app, |rt| rt.bridge = None);
        return;
    }
    let result = (|| -> anyhow::Result<BridgeInfo> {
        let node = bridge::find_node().ok_or_else(|| anyhow::anyhow!("找不到 Node.js,CLI agent 需要先安裝 Node"))?;
        let cfg_path = paths::bridge_config_file();
        let existing = std::fs::read_to_string(&cfg_path).ok();
        let merged = bridge::merge_config(existing.as_deref(), bridge::Providers::default())?;
        if let Some(dir) = cfg_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&cfg_path, &merged)?;
        let (port, token) = bridge::read_endpoint(&merged).ok_or_else(|| anyhow::anyhow!("bridge 設定缺 port 或 token"))?;
        let _ = std::fs::create_dir_all(paths::logs_dir(&st.data));
        let proc = bridge::BridgeProcess::spawn(&node, &st.bridge_script, &paths::logs_dir(&st.data).join("bridge.log"), &st.job)?;
        *st.bridge.lock().unwrap_or_else(|e| e.into_inner()) = Some(proc);
        Ok(BridgeInfo { url: format!("http://127.0.0.1:{port}/mcp"), token })
    })();
    match result {
        Ok(info) => st.update(app, |rt| rt.bridge = Some(info)),
        Err(e) => st.update(app, |rt| {
            rt.bridge = None;
            rt.message = Some(format!("CLI agent bridge 啟動失敗:{e}"));
        }),
    }
}

/// 下載 llama.cpp 與模型(已下載的略過),完成後自動啟動
fn start_download(app: &AppHandle) {
    let st = state(app).inner().clone();
    let app = app.clone();
    std::thread::spawn(move || {
        let jobs: [(Package, PathBuf); 2] = [
            (download::llama_package(), st.data.join("bin")),
            (download::model_package(), paths::models_dir(&st.data)),
        ];
        for (pkg, parent) in jobs {
            if download::package_ready(&pkg, &parent) {
                continue;
            }
            let Some(guard) = st.downloads.try_begin(&pkg.id) else { return };
            st.update(&app, |rt| {
                rt.status = LlamaStatus::Downloading;
                rt.message = Some(format!("下載 {}", pkg.label));
            });
            let ctl = guard.ctl();
            let result = download::check_disk_space(&pkg, &parent).and_then(|_| {
                download::download_package(&pkg, &parent, &DownloadOptions::default(), &ctl, &mut |p| {
                    let _ = app.emit("model-download", p);
                })
            });
            drop(guard);
            if let Err(e) = result {
                st.update(&app, |rt| {
                    rt.status = LlamaStatus::NotDownloaded;
                    rt.message = Some(e.to_string());
                });
                return;
            }
        }
        start_model(&app);
    });
}

#[tauri::command]
fn get_status(app: AppHandle) -> StatusView {
    state(&app).view()
}

#[tauri::command]
fn download_start(app: AppHandle) {
    start_download(&app);
}

#[tauri::command]
fn download_cancel(app: AppHandle) {
    state(&app).downloads.cancel_all();
}

#[tauri::command]
fn model_start(app: AppHandle) {
    start_model(&app);
}

#[tauri::command]
fn model_stop(app: AppHandle) {
    stop_model(&app);
}

#[tauri::command]
fn bridge_set_enabled(app: AppHandle, enabled: bool) {
    let st = state(&app).inner().clone();
    st.settings.lock().unwrap_or_else(|e| e.into_inner()).bridge_enabled = enabled;
    st.save_settings();
    std::thread::spawn(move || apply_bridge(&app));
}

#[tauri::command]
fn open_logs(app: AppHandle) {
    let dir = paths::logs_dir(&state(&app).data);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// llama-server 意外結束時改成錯誤狀態
fn watch_llama(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        let st = state(&app).inner().clone();
        let died = {
            let mut g = st.llama.lock().unwrap_or_else(|e| e.into_inner());
            let exited = g.as_mut().is_some_and(|p| p.exited());
            if exited {
                g.take();
            }
            exited
        };
        if died {
            st.update(&app, |rt| {
                rt.status = LlamaStatus::Error;
                rt.llama = None;
                rt.message = Some("本地模型意外結束,請按「啟動」重試(記錄在 logs\\llama-server.log)".into());
            });
        }
    });
}

fn shutdown_children(app: &AppHandle) {
    let st = state(app).inner().clone();
    if let Some(mut p) = st.llama.lock().unwrap_or_else(|e| e.into_inner()).take() {
        p.shutdown();
    }
    if let Some(mut b) = st.bridge.lock().unwrap_or_else(|e| e.into_inner()).take() {
        b.shutdown();
    }
    let _ = std::fs::remove_file(paths::runtime_file(&st.data));
}

fn resource_bridge(app: &AppHandle) -> PathBuf {
    app.path()
        .resource_dir()
        .map(|d| d.join("bridge.mjs"))
        .ok()
        .filter(|p| p.is_file())
        // 開發期:workspace 的 bridge 產物
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bridge/dist/bridge.mjs"))
}

pub fn run(context: tauri::Context<tauri::Wry>) {
    let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).try_init();
    let background = std::env::args().any(|a| a == "--background");
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 已在執行:喚起視窗;背景啟動(開機、擴充喚起)不打擾使用者
            if !args.iter().any(|a| a == "--background") {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .invoke_handler(tauri::generate_handler![
            get_status,
            download_start,
            download_cancel,
            model_start,
            model_stop,
            bridge_set_enabled,
            open_logs
        ])
        .setup(move |app| {
            let data = paths::data_dir();
            std::fs::create_dir_all(&data)?;
            let settings: AppSettings = std::fs::read_to_string(paths::app_settings_file(&data))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let st = Arc::new(AppState {
                bridge_script: resource_bridge(app.handle()),
                job: Job::new()?,
                runtime: Mutex::new(Runtime {
                    pid: std::process::id(),
                    version: VERSION.into(),
                    status: LlamaStatus::Stopped,
                    llama: None,
                    bridge: None,
                    message: None,
                }),
                llama: Mutex::new(None),
                bridge: Mutex::new(None),
                downloads: Arc::new(DownloadRegistry::default()),
                settings: Mutex::new(settings),
                data,
            });
            app.manage(st.clone());

            if let Ok(exe) = std::env::current_exe() {
                if let Err(e) = register::register(&exe, &paths::native_host_manifest(&st.data)) {
                    tracing::warn!("登錄 native messaging host 失敗:{e}");
                }
            }

            let open = MenuItem::with_id(app, "open", "開啟主視窗", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().ok_or("缺少程式圖示")?)
                .tooltip("agent-translator 本機翻譯")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            let handle = app.handle().clone();
            start_model(&handle);
            std::thread::spawn({
                let handle = handle.clone();
                move || apply_bridge(&handle)
            });
            watch_llama(handle.clone());
            if !background {
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            // 關閉視窗只是縮到系統匣,模型繼續提供翻譯
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .build(context)
        .expect("啟動 Tauri 失敗")
        .run(|app, e| {
            if let RunEvent::Exit = e {
                shutdown_children(app);
            }
        });
}
