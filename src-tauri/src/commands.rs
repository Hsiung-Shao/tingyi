//! 前端可呼叫的命令。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tingyi_core::asr::Lang;
use tingyi_core::audio::source::{self, AudioSource, SourceEntry};
use tingyi_core::cleanup::Replacement;
use tingyi_core::llama::{WhisperServerProcess, WhisperServerSpec};
use tingyi_core::translate_server::{Endpoint, Holder, SUBTITLE_SLOT};
use tingyi_core::pipeline::AsrEngine;
use tingyi_core::mt::{LlamaServerConfig, LlamaServerTranslator, Translator};
use tingyi_core::pipeline::{self, PipelineConfig};
use tingyi_core::postprocess::{Glossary, GlossaryEntry};
use tingyi_core::export::{self, ExportSentence};
use tingyi_core::idle::IdleTracker;
use tingyi_core::model_download::{self as md, DownloadError, DownloadOptions, ModelStatus, Phase, Progress};
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

/// 這台機器的預設翻譯模型;網頁翻譯在共用伺服器沒在跑時用它。
pub(crate) fn default_mt_model() -> String {
    recommend_model(gpu::nvidia_info().map(|g| g.1))
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
            id: "routed".into(),
            label: "專用模型(CPU,日文 ReazonSpeech / 英文 Parakeet)".into(),
            present: paths::sensevoice_model().exists()
                && md::MODELS.iter().any(|m| md::model_status(m, &paths::models_dir()) == ModelStatus::Ready),
            note: "不佔顯示記憶體;韓文仍用 SenseVoice;模型在下方下載".into(),
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
    /// 辨識引擎:"sensevoice"(預設)、"routed" 或 "whisper-turbo"。
    pub asr_engine: Option<String>,
    /// 辨識取代表(錯字 → 正字)。
    pub replacements: Option<Vec<Replacement>>,
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

    // 1. 翻譯模型(可選):字幕與瀏覽器擴充共用同一個 llama-server
    let mut translator: Option<Arc<dyn Translator>> = None;
    if let Some(model) = opts.model.as_deref().filter(|m| !m.is_empty()) {
        emit_status(&format!("啟動 llama-server({model})…"));
        let before = gpu::nvidia_used_mib();
        let slot = state.translate.clone();
        let model_name = model.to_string();
        let ep = tokio::task::spawn_blocking(move || -> anyhow::Result<(Endpoint, Option<u32>)> {
            let mut g = crate::state::lock_translate(&slot)?;
            let s = g.as_mut().expect("lock_translate 已建立");
            let ep = s.acquire(Holder::Subtitles, &model_name, paths::gguf_model(&model_name))?;
            Ok((ep, s.pid()))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
        let (ep, server_pid) = ep;
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
        let mut mt_cfg = LlamaServerConfig::for_model(ep.base_url.clone(), model);
        mt_cfg.api_key = Some(ep.api_key.clone());
        let tr = LlamaServerTranslator::new(mt_cfg);
        // 熱身:固定前綴進快取
        let mut noop = |_: &str| {};
        let _ = tr
            .translate(
                tingyi_core::mt::TranslateRequest {
                    text: "Hello.".into(),
                    src_lang: Lang::En,
                    slot: Some(SUBTITLE_SLOT),
                    ..Default::default()
                },
                &mut noop,
            )
            .await;
        // 子程序 pid 立刻登錄到取樣器,不等前端拉 get_metrics
        if let Some(pid) = server_pid {
            state.watched.lock().unwrap().push(("llama-server".into(), pid));
        }
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
            release_subtitles_blocking(&state.translate);
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
    cfg.audio_source = state.audio_source.clone();
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
    if let Some(r) = opts.replacements {
        cfg.replacements = r;
    }
    if opts.asr_engine.as_deref() == Some("routed") {
        cfg.asr_engine = AsrEngine::Routed;
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
            release_subtitles_blocking(&state.translate);
            if let Some(mut w) = state.whisper.lock().unwrap().take() {
                w.shutdown();
            }
            return Err(e);
        }
    };

    // 3. 事件轉發;順便判斷懸浮字幕閒置(沒聲音且沒有新內容)→ 通知字幕視窗收起
    let app2 = app.clone();
    let stats = handle.stats();
    let idle_control = state.idle_control.clone();
    let translate_on = translator.is_some();
    let fwd = tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        use std::time::Instant;
        let mut idle = IdleTracker::new(translate_on, Instant::now());
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                ev = rx.recv() => {
                    let Some(ev) = ev else { break };
                    idle.on_event(&ev, Instant::now());
                    let _ = app2.emit("pipeline", &ev);
                }
                _ = tick.tick() => {
                    let hide = Duration::from_millis(idle_control.hide_ms.load(Ordering::Relaxed) as u64);
                    let speaking = stats.speaking.load(Ordering::Relaxed);
                    let silence = Duration::from_millis(stats.silence_ms());
                    let packets = stats.capture.packets.load(Ordering::Relaxed);
                    if idle.poll(Instant::now(), hide, speaking, silence, packets) {
                        let _ = app2.emit("subtitle-idle", ());
                    }
                }
            }
        }
    });
    *state.forwarder.lock().unwrap() = Some(fwd);
    *state.pipeline.lock().unwrap() = Some(handle);
    emit_status("執行中");
    let _ = app.emit("session-running", true);
    if let Err(e) = keep_main_focus(&app, || set_overlay_visible(&app, true)) {
        tracing::warn!("顯示懸浮字幕失敗:{e}");
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_session(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("停止回合(stop_session)");
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
    let slot = state.translate.clone();
    let still_serving = tokio::task::spawn_blocking(move || release_subtitles(&slot)).await.unwrap_or(false);
    let _ = app.emit("session-running", false);
    let _ = app.emit(
        "status",
        if still_serving { "已停止(翻譯伺服器仍供網頁翻譯使用)" } else { "已停止" }.to_string(),
    );
    Ok(())
}

/// 失敗路徑也要放掉字幕;放手可能切回網頁的模型(要數秒),不在 async 執行緒上等。
fn release_subtitles_blocking(slot: &crate::state::TranslateSlot) {
    let slot = slot.clone();
    tokio::task::spawn_blocking(move || release_subtitles(&slot));
}

/// 字幕不再使用共用翻譯伺服器(網頁還在用時切回網頁的模型);回傳伺服器是否仍在執行。
fn release_subtitles(slot: &crate::state::TranslateSlot) -> bool {
    let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_mut() {
        Some(s) => {
            s.release(Holder::Subtitles);
            s.is_running()
        }
        None => false,
    }
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

/// 目前有音訊 session 的程式(排除本程式)。列舉裝置與程序要數百毫秒,放到背景執行緒。
#[tauri::command]
pub async fn list_audio_apps() -> Result<Vec<SourceEntry>, String> {
    tokio::task::spawn_blocking(source::list_audio_sources)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
pub struct AudioSourceState {
    pub source: AudioSource,
    /// 指定程式已結束,等它重新開啟中。
    pub waiting: bool,
}

/// 切換音訊來源;執行中由擷取執行緒在下一輪(約 100 ms 內)重建 session。
#[tauri::command]
pub fn set_audio_source(state: State<'_, AppState>, source: AudioSource) {
    tracing::info!("音訊來源 → {source:?}");
    state.audio_source.set_waiting(false);
    state.audio_source.set(source);
}

#[tauri::command]
pub fn get_audio_source(state: State<'_, AppState>) -> AudioSourceState {
    AudioSourceState {
        source: state.audio_source.get().0,
        waiting: state.audio_source.waiting(),
    }
}

/// 一個專用模型的狀態(下載卡用)。
#[derive(Serialize)]
pub struct RoutedModelState {
    pub id: String,
    pub label: String,
    pub lang: Lang,
    pub size_bytes: u64,
    pub ready: bool,
    pub downloading: bool,
    /// 已下載位元組:下載中為即時進度;中斷過則是可續傳的起點。
    pub done: u64,
    pub license: String,
    pub license_url: String,
}

/// 專用模型的就緒與下載狀態;只讀檔案大小,不呼叫 nvidia-smi(前端掛載與重載時查一次)。
#[tauri::command]
pub async fn routed_models_status(state: State<'_, AppState>) -> Result<Vec<RoutedModelState>, String> {
    packages_status(state.downloads.clone(), md::MODELS).await
}

/// 翻譯用套件(llama.cpp、Hy-MT2 1.8B)的就緒與下載狀態;安裝版靠它在 app 內下載。
#[tauri::command]
pub async fn translate_packages_status(state: State<'_, AppState>) -> Result<Vec<RoutedModelState>, String> {
    packages_status(state.downloads.clone(), md::TRANSLATE_PACKAGES).await
}

async fn packages_status(
    registry: Arc<md::DownloadRegistry>,
    specs: &'static [md::ModelSpec],
) -> Result<Vec<RoutedModelState>, String> {
    tokio::task::spawn_blocking(move || {
        specs
            .iter()
            .map(|m| {
                let dir = md::install_parent(m);
                let total = m.total_size();
                let snapshot = registry.snapshot(m.id);
                RoutedModelState {
                    id: m.id.into(),
                    label: m.label.into(),
                    lang: m.lang,
                    size_bytes: total,
                    // 腳本放的位置(zip 在 bin 根層、gguf 在 models 根層)也算已下載
                    ready: md::package_usable(m, &paths::data_dir()),
                    downloading: snapshot.is_some(),
                    done: snapshot.map(|(d, _)| d).unwrap_or_else(|| total.saturating_sub(md::bytes_needed(m, &dir))),
                    license: m.license.into(),
                    license_url: m.license_url.into(),
                }
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// 開始下載一個專用模型(背景執行緒;進度以 `model-download` 事件推給主視窗)。
/// 同一個模型已在下載中或磁碟空間不足時立即回錯;日文與英文可同時下載。
#[tauri::command]
pub async fn download_model(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let spec = md::spec(&id).ok_or_else(|| format!("未知的模型:{id}"))?;
    if md::package_usable(spec, &paths::data_dir()) {
        // 已經能用(含腳本放的位置)就不重下;llama.cpp 執行中時重新解壓也會覆寫失敗
        return Ok(());
    }
    let guard = state.downloads.try_begin(spec.id).ok_or("此模型已在下載中")?;
    // llama.cpp 裝在 bin/,其餘裝在 models/
    let models_dir = md::install_parent(spec);
    let dir2 = models_dir.clone();
    tokio::task::spawn_blocking(move || md::check_disk_space(spec, &dir2))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name(format!("tingyi-download-{id}"))
        .spawn(move || {
            let ctl = guard.ctl();
            let emit = |p: &Progress| {
                let _ = app.emit_to("main", "model-download", p);
            };
            // Done 由下面解除登記後統一發,core 那則不轉發
            let result = md::download_model(spec, &models_dir, &DownloadOptions::default(), &ctl, &mut |p| {
                if p.phase != Phase::Done {
                    emit(&p);
                }
            });
            // 先解除登記再發最後一則事件,前端收到後查狀態才不會看到「下載中」
            drop(guard);
            let (phase, message) = match &result {
                Ok(()) => (Phase::Done, None),
                Err(DownloadError::Cancelled) => (Phase::Cancelled, None),
                Err(e) => (Phase::Error, Some(e.to_string())),
            };
            if let Err(e) = &result {
                tracing::warn!("下載 {} 未完成:{e}", spec.id);
            }
            // 翻譯套件下載完:讓擴充下次連線就能喚起網頁翻譯,不必重開程式
            if result.is_ok() && md::TRANSLATE_PACKAGES.iter().any(|p| p.id == spec.id) {
                crate::web::refresh_after_download(&app);
            }
            emit(&Progress {
                id: spec.id.into(),
                phase,
                done: ctl.done.load(std::sync::atomic::Ordering::Relaxed),
                total: spec.total_size(),
                bps: 0,
                message,
            });
        })
        .map_err(|e| format!("無法開始下載:{e}"))?;
    Ok(())
}

/// 取消下載;暫存檔保留,下次從斷點續傳。
#[tauri::command]
pub fn cancel_model_download(state: State<'_, AppState>, id: String) -> bool {
    state.downloads.cancel(&id)
}

/// 效能指標:系統 / 子程序資源 + 管線計數。前端每 2 s 拉一次;第一次呼叫才啟動取樣器。
#[tauri::command]
pub fn get_metrics(state: State<'_, AppState>) -> Metrics {
    ensure_sampler(&state);
    // 同步子程序 pid 清單
    {
        let mut w = state.watched.lock().unwrap();
        w.clear();
        // 啟動中會在鎖內等就緒;拿不到鎖就這輪不列,不卡住前端
        if let Some(pid) = state.translate.try_lock().ok().and_then(|g| g.as_ref().and_then(|s| s.pid())) {
            w.push(("llama-server".into(), pid));
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

/// 斷句靈敏度:fast / normal / slow(−100 / 0 / +100 ms)、是否自動調整、手動單段上限(秒,None = 依靈敏度)。
/// 執行中即時生效。
#[tauri::command]
pub fn set_segmentation(
    state: State<'_, AppState>,
    sensitivity: String,
    adaptive: bool,
    max_speech_sec: Option<f32>,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let offset = match sensitivity.as_str() {
        "fast" => -100,
        "slow" => 100,
        _ => 0,
    };
    let override_ms = max_speech_sec.filter(|s| s.is_finite() && *s > 0.0).map_or(0, |s| (s * 1000.0).round() as u32);
    state.seg_control.offset_ms.store(offset, Ordering::Relaxed);
    state.seg_control.adaptive.store(adaptive, Ordering::Relaxed);
    state.seg_control.max_speech_override_ms.store(override_ms, Ordering::Relaxed);
    Ok(())
}

/// 懸浮字幕沒聲音幾秒後收起;0 = 不隱藏。執行中改了立即生效;回傳夾限後實際生效的秒數。
#[tauri::command]
pub fn set_subtitle_hide(state: State<'_, AppState>, seconds: f32) -> f32 {
    state.idle_control.set_secs(seconds) as f32 / 1000.0
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

#[tauri::command]
pub fn web_status(app: AppHandle) -> crate::web::WebStatus {
    crate::web::status(&app)
}

#[tauri::command]
pub fn web_start(app: AppHandle) {
    crate::web::serve_web(&app);
}

#[tauri::command]
pub fn web_stop(app: AppHandle) {
    crate::web::stop_web(&app);
}

#[tauri::command]
pub fn set_web_model(app: AppHandle, model: String) -> Result<(), String> {
    crate::web::set_web_model(&app, model).map_err(|e| format!("{e:#}"))
}

/// 主視窗的開始選項(沒存過回 None,前端用建議模型)。
#[tauri::command]
pub fn get_prefs() -> Option<crate::settings::Prefs> {
    crate::settings::load(&crate::web::settings_file()).prefs
}

#[tauri::command]
pub fn save_prefs(prefs: crate::settings::Prefs) -> Result<(), String> {
    let path = crate::web::settings_file();
    let mut s = crate::settings::load(&path);
    s.prefs = Some(prefs);
    crate::settings::save(&path, &s).map_err(|e| e.to_string())
}
