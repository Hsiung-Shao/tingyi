//! 前端可呼叫的命令。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tingyi_core::asr::Lang;
use tingyi_core::llama::{LlamaServerProcess, LlamaServerSpec, WhisperServerProcess, WhisperServerSpec};
use tingyi_core::pipeline::AsrEngine;
use tingyi_core::mt::{LlamaServerConfig, LlamaServerTranslator, Translator};
use tingyi_core::pipeline::{self, PipelineConfig};
use tingyi_core::postprocess::{Glossary, GlossaryEntry};
use tingyi_core::export::{self, ExportSentence};
use tingyi_core::{gpu, paths};

use crate::state::AppState;

#[derive(Serialize)]
pub struct Info {
    pub data_dir: String,
    pub device: Option<String>,
    pub gpu: Option<(String, u32)>,
    pub gpu_used_mib: Option<u32>,
    pub models: Vec<ModelInfo>,
    pub sensevoice_ready: bool,
    pub vad_ready: bool,
    pub llama_server_ready: bool,
    pub running: bool,
    /// 開發用:環境變數 `TINGYI_AUTOSTART=1` 時前端載入後自動按「開始」。
    pub autostart: bool,
    /// 依顯示記憶體與已下載檔案建議的翻譯模型。
    pub recommended_model: String,
    /// 可用的辨識引擎。
    pub asr_engines: Vec<AsrEngineInfo>,
    pub speaker_model_ready: bool,
    /// 目前斷句設定(偏移 ms、自動、生效的靜音門檻 ms)。
    pub seg_offset_ms: i32,
    pub seg_adaptive: bool,
    pub seg_current_ms: u32,
}

#[derive(Serialize)]
pub struct AsrEngineInfo {
    pub id: String,
    pub label: String,
    pub present: bool,
    pub note: String,
}

/// 總顯示記憶體 ≥ 8 GB 且 7B 檔存在 → 7B Q4;≥ 4 GB → 1.8B Q8;否則 1.8B Q4。
/// 使用者實測(2026-10-06):7B Q4 精準度明顯較好,首字僅多約 50 ms。
fn recommend_model(total_mib: Option<u32>) -> String {
    let present = |n: &str| paths::gguf_model(n).exists();
    match total_mib {
        Some(t) if t >= 8000 && present("Hy-MT2-7B-Q4_K_M") => "Hy-MT2-7B-Q4_K_M".into(),
        Some(t) if t >= 4000 && present("Hy-MT2-1.8B-Q8_0") => "Hy-MT2-1.8B-Q8_0".into(),
        _ if present("Hy-MT2-1.8B-Q4_K_M") => "Hy-MT2-1.8B-Q4_K_M".into(),
        _ => "Hy-MT2-1.8B-Q8_0".into(),
    }
}

#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub note: String,
    pub present: bool,
    pub size_mb: u64,
}

/// 狀態資訊。改為非同步命令:同步命令在 Tauri 2 跑在主執行緒,
/// 這裡會呼叫 nvidia-smi 與列舉音訊裝置(數百毫秒),放在主執行緒會讓整個介面卡住。
#[tauri::command]
pub async fn get_info(state: State<'_, AppState>) -> Result<Info, String> {
    let seg_offset_ms = state.seg_control.offset_ms.load(std::sync::atomic::Ordering::Relaxed);
    let seg_adaptive = state.seg_control.adaptive.load(std::sync::atomic::Ordering::Relaxed);
    let seg_current_ms = state.seg_control.current_ms.load(std::sync::atomic::Ordering::Relaxed);
    let running = state.is_running();
    tokio::task::spawn_blocking(move || collect_info(seg_offset_ms, seg_adaptive, seg_current_ms, running))
        .await
        .map_err(|e| e.to_string())
}

fn collect_info(seg_offset_ms: i32, seg_adaptive: bool, seg_current_ms: u32, running: bool) -> Info {
    let models = paths::KNOWN_MT_MODELS
        .iter()
        .map(|(name, note)| {
            let p = paths::gguf_model(name);
            let size_mb = std::fs::metadata(&p).map(|m| m.len() / 1_000_000).unwrap_or(0);
            ModelInfo {
                name: name.to_string(),
                note: note.to_string(),
                present: p.exists(),
                size_mb,
            }
        })
        .collect();
    let gpu_info = gpu::nvidia_info();
    let recommended_model = recommend_model(gpu_info.as_ref().map(|g| g.1));
    let whisper_present = paths::whisper_server_exe().exists() && paths::whisper_model().exists();
    let asr_engines = vec![
        AsrEngineInfo {
            id: "sensevoice".into(),
            label: "SenseVoice-Small(CPU,快)".into(),
            present: paths::sensevoice_model().exists(),
            note: "預設;不佔顯示記憶體".into(),
        },
        AsrEngineInfo {
            id: "whisper-turbo".into(),
            label: "Whisper large-v3-turbo(GPU,準)".into(),
            present: whisper_present,
            note: if whisper_present {
                "顯示記憶體約 +1.5 GB;預覽仍用 SenseVoice".into()
            } else {
                "需先執行 scripts/build-whisper.ps1 並下載模型".into()
            },
        },
    ];
    Info {
        data_dir: paths::data_dir().display().to_string(),
        device: tingyi_core::audio::capture::default_render_device_name().ok(),
        gpu: gpu_info,
        recommended_model,
        asr_engines,
        speaker_model_ready: paths::speaker_model().exists(),
        seg_offset_ms,
        seg_adaptive,
        seg_current_ms,
        gpu_used_mib: gpu::nvidia_used_mib(),
        models,
        sensevoice_ready: paths::sensevoice_model().exists() && paths::sensevoice_tokens().exists(),
        vad_ready: paths::silero_vad_model().exists(),
        llama_server_ready: paths::llama_server_exe().exists(),
        running,
        autostart: std::env::var("TINGYI_AUTOSTART").map(|v| v == "1").unwrap_or(false),
    }
}

#[derive(Deserialize)]
pub struct StartOptions {
    /// 翻譯模型名稱;None 或空字串 = 只辨識不翻譯。
    pub model: Option<String>,
    /// 辨識引擎:"sensevoice"(預設)或 "whisper-turbo"。
    pub asr_engine: Option<String>,
    /// 說話者辨識(預設開;模型不存在時自動略過)。
    pub speaker_id: Option<bool>,
    /// 開始時是否清除上一輪的字幕紀錄(預設清除;前端有舊紀錄時會先詢問)。
    pub clear_history: Option<bool>,
    /// 鎖定來源語言 en/ja/ko;None = 自動(限 en/ja/ko)。
    pub lang: Option<String>,
    pub glossary: Option<Vec<GlossaryEntry>>,
    pub port: Option<u16>,
    pub asr_threads: Option<i32>,
    /// 回修(慢路徑):幾秒後用更長的音訊重新辨識並重翻;僅 Whisper 引擎生效(預設開)。
    pub refine: Option<bool>,
}

fn parse_lang(s: &str) -> Option<Lang> {
    match s {
        "en" => Some(Lang::En),
        "ja" => Some(Lang::Ja),
        "ko" => Some(Lang::Ko),
        _ => None,
    }
}

#[tauri::command]
pub async fn start_session(
    app: AppHandle,
    state: State<'_, AppState>,
    opts: StartOptions,
) -> Result<(), String> {
    if state.is_running() {
        return Err("已在執行中".into());
    }
    let port = opts.port.unwrap_or(8178);
    let emit_status = |msg: &str| {
        let _ = app.emit("status", msg.to_string());
    };
    // 新回合:前端清空上一輪的字幕列表(使用者選擇保留時不清)
    if opts.clear_history.unwrap_or(true) {
        let _ = app.emit("session-start", ());
    }

    // 回修只在 Whisper 引擎下生效;決定 llama-server 要不要開兩個 slot
    let refine_on = opts.refine.unwrap_or(true) && opts.asr_engine.as_deref() == Some("whisper-turbo");

    // 1. 翻譯模型(可選)
    let mut translator: Option<Arc<dyn Translator>> = None;
    if let Some(model) = opts.model.as_deref().filter(|m| !m.is_empty()) {
        emit_status(&format!("啟動 llama-server({model})…"));
        let mut spec = LlamaServerSpec::new(paths::gguf_model(model), port);
        if refine_on {
            spec = spec.with_refine();
        }
        spec.log_file = Some(paths::data_dir().join("llama-server.log"));
        let before = gpu::nvidia_used_mib();
        let proc_ = tokio::task::spawn_blocking(move || -> anyhow::Result<LlamaServerProcess> {
            let mut p = LlamaServerProcess::spawn(spec)?;
            p.wait_ready_blocking(Duration::from_secs(180))?;
            Ok(p)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
        let after = gpu::nvidia_used_mib();
        if let (Some(a), Some(b)) = (before, after) {
            emit_status(&format!(
                "llama-server 就緒,顯示記憶體 +{} MiB(總 {} MiB)",
                b.saturating_sub(a),
                b
            ));
        } else {
            emit_status("llama-server 就緒");
        }
        let tr = LlamaServerTranslator::new(LlamaServerConfig::for_model(
            format!("http://127.0.0.1:{port}"),
            model,
        ));
        // 熱身:固定前綴進快取
        let mut noop = |_: &str| {};
        let _ = tr
            .translate(
                tingyi_core::mt::TranslateRequest {
                    text: "Hello.".into(),
                    src_lang: Lang::En,
                    slot: if refine_on { Some(0) } else { None },
                    ..Default::default()
                },
                &mut noop,
            )
            .await;
        // 子程序 pid 立刻登錄到取樣器,不等前端拉 get_metrics
        state.watched.lock().unwrap().push(("llama-server".into(), proc_.pid()));
        *state.server.lock().unwrap() = Some(proc_);
        translator = Some(Arc::new(tr));
    }

    // 取樣器隨回合啟動,匯出診斷時才有資料
    ensure_sampler(&state);

    // 1b. Whisper 辨識引擎(可選)
    let mut cfg = PipelineConfig::default();
    if opts.asr_engine.as_deref() == Some("whisper-turbo") {
        emit_status("啟動 whisper-server(large-v3-turbo)…");
        let wport = port + 1;
        let mut spec = WhisperServerSpec::new(paths::whisper_model(), wport);
        spec.log_file = Some(paths::data_dir().join("whisper-server.log"));
        let before = gpu::nvidia_used_mib();
        let wp = tokio::task::spawn_blocking(move || -> anyhow::Result<WhisperServerProcess> {
            let mut p = WhisperServerProcess::spawn(spec)?;
            p.wait_ready_blocking(Duration::from_secs(120))?;
            Ok(p)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            if let Some(mut s) = state.server.lock().unwrap().take() {
                s.shutdown();
            }
            format!("{e:#}")
        })?;
        let after = gpu::nvidia_used_mib();
        if let (Some(a), Some(b)) = (before, after) {
            emit_status(&format!("whisper-server 就緒,顯示記憶體 +{} MiB", b.saturating_sub(a)));
        }
        cfg.asr_engine = AsrEngine::Whisper {
            base_url: wp.base_url(),
        };
        state.watched.lock().unwrap().push(("whisper-server".into(), wp.pid()));
        *state.whisper.lock().unwrap() = Some(wp);
    }

    // 2. 管線
    cfg.seg_control = state.seg_control.clone();
    cfg.speaker_id = opts.speaker_id.unwrap_or(true);
    cfg.refine.enabled = refine_on;
    cfg.lock_lang = opts.lang.as_deref().and_then(parse_lang);
    if cfg.lock_lang.is_some() {
        cfg.allowed_langs = HashSet::new();
    }
    if let Some(t) = opts.asr_threads {
        cfg.asr.num_threads = t;
    }
    if let Some(g) = opts.glossary {
        cfg.glossary = Glossary { entries: g };
    }
    emit_status("載入 SenseVoice 與 VAD…");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg2 = cfg.clone();
    let tr2 = translator.clone();
    let handle = tokio::task::spawn_blocking(move || pipeline::start(cfg2, tr2, tx))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            // 失敗要把 server 一起關掉
            format!("{e:#}")
        });
    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            if let Some(mut s) = state.server.lock().unwrap().take() {
                s.shutdown();
            }
            if let Some(mut w) = state.whisper.lock().unwrap().take() {
                w.shutdown();
            }
            return Err(e);
        }
    };

    // 3. 事件轉發
    let app2 = app.clone();
    let fwd = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app2.emit("pipeline", &ev);
        }
    });
    *state.forwarder.lock().unwrap() = Some(fwd);
    *state.pipeline.lock().unwrap() = Some(handle);
    emit_status("執行中");
    if let Err(e) = keep_main_focus(&app, || set_overlay_visible(&app, true)) {
        tracing::warn!("顯示懸浮字幕失敗:{e}");
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_session(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // 按下停止就先收起字幕(關子程序要幾秒),並退出拖曳模式恢復滑鼠穿透
    keep_main_focus(&app, || {
        if let Some(w) = app.get_webview_window("overlay") {
            let _ = w.set_ignore_cursor_events(true);
        }
        if let Err(e) = set_overlay_visible(&app, false) {
            tracing::warn!("隱藏懸浮字幕失敗:{e}");
        }
    });
    let _ = app.emit("overlay-interactive", false);
    let handle = state.pipeline.lock().unwrap().take();
    if let Some(h) = handle {
        h.stop().await;
    }
    if let Some(f) = state.forwarder.lock().unwrap().take() {
        f.abort();
    }
    let whisper = state.whisper.lock().unwrap().take();
    if let Some(mut w) = whisper {
        let _ = tokio::task::spawn_blocking(move || w.shutdown()).await;
    }
    let server = state.server.lock().unwrap().take();
    if let Some(mut s) = server {
        let ok = tokio::task::spawn_blocking(move || s.shutdown())
            .await
            .unwrap_or(false);
        let _ = app.emit(
            "status",
            if ok { "已停止(llama-server 已結束)" } else { "已停止(llama-server 未確認結束)" }.to_string(),
        );
    } else {
        let _ = app.emit("status", "已停止".to_string());
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Metrics {
    pub system: Option<tingyi_core::sysmon::SysSample>,
    pub pipeline: Option<tingyi_core::metrics::StatsSnapshot>,
    pub running: bool,
}

fn ensure_sampler(state: &AppState) {
    let mut s = state.sampler.lock().unwrap();
    if s.is_none() {
        *s = Some(tingyi_core::sysmon::Sampler::start(
            Duration::from_secs(2),
            150,
            state.watched.clone(),
        ));
    }
}

/// 效能指標:系統 / 子程序資源 + 管線計數。前端每 2 s 拉一次;第一次呼叫才啟動取樣器。
#[tauri::command]
pub fn get_metrics(state: State<'_, AppState>) -> Metrics {
    ensure_sampler(&state);
    // 同步子程序 pid 清單
    {
        let mut w = state.watched.lock().unwrap();
        w.clear();
        if let Some(p) = state.server.lock().unwrap().as_ref() {
            w.push(("llama-server".into(), p.pid()));
        }
        if let Some(p) = state.whisper.lock().unwrap().as_ref() {
            w.push(("whisper-server".into(), p.pid()));
        }
    }
    let system = state.sampler.lock().unwrap().as_ref().and_then(|s| s.latest());
    let pipeline = state
        .pipeline
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.stats().snapshot());
    Metrics {
        system,
        pipeline,
        running: state.is_running(),
    }
}

/// 匯出診斷:最近 5 分鐘取樣 + 管線統計 + 機器資訊 → bench-out/diag-<時間>.json;回傳路徑。
#[tauri::command]
pub fn export_diagnostics(state: State<'_, AppState>) -> Result<String, String> {
    ensure_sampler(&state);
    let history = state
        .sampler
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.history())
        .unwrap_or_default();
    let pipeline = state
        .pipeline
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.stats().snapshot());
    let info = serde_json::json!({
        "gpu": gpu::nvidia_info(),
        "data_dir": paths::data_dir().display().to_string(),
        "os": std::env::consts::OS,
        "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "exported_at": chrono_like_now(),
    });
    let doc = serde_json::json!({ "info": info, "pipeline": pipeline, "samples": history });
    let dir = paths::data_dir().parent().map(|p| p.join("bench-out")).unwrap_or_else(|| paths::data_dir().join("bench-out"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("diag-{}.json", chrono_like_now().replace([':', ' '], "-")));
    std::fs::write(&path, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// 不引入 chrono:以 Unix 秒數組成可讀時間戳。
fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// 斷句靈敏度:fast / normal / slow(−100 / 0 / +100 ms)與是否自動調整。執行中即時生效。
#[tauri::command]
pub fn set_segmentation(state: State<'_, AppState>, sensitivity: String, adaptive: bool) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let offset = match sensitivity.as_str() {
        "fast" => -100,
        "slow" => 100,
        _ => 0,
    };
    state.seg_control.offset_ms.store(offset, Ordering::Relaxed);
    state.seg_control.adaptive.store(adaptive, Ordering::Relaxed);
    Ok(())
}

/// 說話者改名(執行中才有效)。
#[tauri::command]
pub fn rename_speaker(state: State<'_, AppState>, from: String, to: String) -> Result<bool, String> {
    let guard = state.pipeline.lock().unwrap();
    match guard.as_ref() {
        Some(h) => Ok(h.rename_speaker(&from, &to)),
        None => Err("未在執行中".into()),
    }
}

#[tauri::command]
pub fn list_speakers(state: State<'_, AppState>) -> Vec<String> {
    state
        .pipeline
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.speakers())
        .unwrap_or_default()
}

/// 解鎖/鎖定懸浮字幕(鎖定 = 滑鼠穿透)。
#[tauri::command]
pub fn set_overlay_interactive(app: AppHandle, state: State<'_, AppState>, interactive: bool) -> Result<(), String> {
    let w = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    // 停止狀態也要能調整位置:解鎖時先叫出字幕(失敗就直接回錯、不動穿透),
    // 鎖定時先切穿透,若沒在執行最後再收起來;全部成功才 emit。
    if interactive {
        // 解鎖:使用者接著要拖字幕,不還焦點
        set_overlay_visible(&app, true)?;
        w.set_ignore_cursor_events(false)
            .map_err(|e| e.to_string())?;
    } else {
        keep_main_focus(&app, || -> Result<(), String> {
            w.set_ignore_cursor_events(true)
                .map_err(|e| e.to_string())?;
            if !state.is_running() {
                set_overlay_visible(&app, false)?;
            }
            Ok(())
        })?;
    }
    let _ = app.emit("overlay-interactive", interactive);
    Ok(())
}

/// 該視窗是否為前景視窗。
/// 不用 is_focused:單一 webview 視窗的鍵盤焦點在 WebView2 子視窗,頂層視窗幾乎永遠回 false。
#[cfg(windows)]
fn is_foreground(w: &tauri::WebviewWindow) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> *mut std::ffi::c_void;
    }
    let fg = unsafe { GetForegroundWindow() };
    match w.hwnd() {
        Ok(h) => h.0 as *mut std::ffi::c_void == fg,
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn is_foreground(w: &tauri::WebviewWindow) -> bool {
    w.is_focused().unwrap_or(false)
}

/// 字幕視窗 show / 切換穿透時 tao 會用 SW_SHOW 啟用它、搶走主視窗焦點;
/// 這些操作都源自使用者在主視窗按鈕,所以若主視窗原本在前景,做完就把焦點還給它。
fn keep_main_focus<T>(app: &AppHandle, f: impl FnOnce() -> T) -> T {
    let main = app.get_webview_window("main");
    let had = main.as_ref().map(is_foreground).unwrap_or(false);
    let r = f();
    // 做完後前景仍是主視窗或字幕視窗才還焦點,避免把使用者切去別的程式又拉回來
    let still_ours = had
        && (main.as_ref().map(is_foreground).unwrap_or(false)
            || app
                .get_webview_window("overlay")
                .as_ref()
                .map(is_foreground)
                .unwrap_or(false));
    if still_ours {
        if let Some(m) = main {
            if let Err(e) = m.set_focus() {
                tracing::warn!("還原主視窗焦點失敗:{e}");
            }
        }
    }
    r
}

/// 懸浮字幕顯示 / 隱藏(開始時顯示、停止時隱藏;`show_overlay` 命令也走這裡)。
fn set_overlay_visible(app: &AppHandle, visible: bool) -> Result<(), String> {
    let w = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    if visible {
        w.show().map_err(|e| e.to_string())?;
        // 顯示後工作列按鈕可能重現,再強制隱藏一次
        if let Err(e) = w.set_skip_taskbar(true) {
            tracing::warn!("overlay set_skip_taskbar 失敗:{e}");
        }
        Ok(())
    } else {
        w.hide().map_err(|e| e.to_string())
    }
}

/// 顯示模式:bilingual / translation。
#[tauri::command]
pub fn set_overlay_mode(app: AppHandle, mode: String) -> Result<(), String> {
    app.emit("overlay-mode", mode).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_overlay(app: AppHandle, visible: bool) -> Result<(), String> {
    set_overlay_visible(&app, visible)
}

/// 匯出辨識紀錄(純文字對照,UTF-8 帶 BOM);回寫入的句數。路徑由前端另存新檔對話框取得。
#[tauri::command]
pub async fn export_transcript(path: String, sentences: Vec<ExportSentence>, exported_at: String) -> Result<usize, String> {
    let n = sentences.len();
    tokio::task::spawn_blocking(move || {
        let text = export::to_text(&sentences, &exported_at);
        export::write_text_file(std::path::Path::new(&path), &text)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"))?;
    Ok(n)
}
