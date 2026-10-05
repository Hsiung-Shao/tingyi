//! 前端可呼叫的命令。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tingyi_core::asr::Lang;
use tingyi_core::llama::{LlamaServerProcess, LlamaServerSpec};
use tingyi_core::mt::{LlamaServerConfig, LlamaServerTranslator, Translator};
use tingyi_core::pipeline::{self, PipelineConfig};
use tingyi_core::postprocess::{Glossary, GlossaryEntry};
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
}

#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub note: String,
    pub present: bool,
    pub size_mb: u64,
}

#[tauri::command]
pub fn get_info(state: State<'_, AppState>) -> Info {
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
    Info {
        data_dir: paths::data_dir().display().to_string(),
        device: tingyi_core::audio::capture::default_render_device_name().ok(),
        gpu: gpu::nvidia_info(),
        gpu_used_mib: gpu::nvidia_used_mib(),
        models,
        sensevoice_ready: paths::sensevoice_model().exists() && paths::sensevoice_tokens().exists(),
        vad_ready: paths::silero_vad_model().exists(),
        llama_server_ready: paths::llama_server_exe().exists(),
        running: state.is_running(),
        autostart: std::env::var("TINGYI_AUTOSTART").map(|v| v == "1").unwrap_or(false),
    }
}

#[derive(Deserialize)]
pub struct StartOptions {
    /// 翻譯模型名稱;None 或空字串 = 只辨識不翻譯。
    pub model: Option<String>,
    /// 鎖定來源語言 en/ja/ko;None = 自動(限 en/ja/ko)。
    pub lang: Option<String>,
    pub glossary: Option<Vec<GlossaryEntry>>,
    pub port: Option<u16>,
    pub asr_threads: Option<i32>,
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

    // 1. 翻譯模型(可選)
    let mut translator: Option<Arc<dyn Translator>> = None;
    if let Some(model) = opts.model.as_deref().filter(|m| !m.is_empty()) {
        emit_status(&format!("啟動 llama-server({model})…"));
        let mut spec = LlamaServerSpec::new(paths::gguf_model(model), port);
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
        let tr = LlamaServerTranslator::new(LlamaServerConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        });
        // 熱身:固定前綴進快取
        let mut noop = |_: &str| {};
        let _ = tr
            .translate(
                tingyi_core::mt::TranslateRequest {
                    text: "Hello.".into(),
                    src_lang: Lang::En,
                    ..Default::default()
                },
                &mut noop,
            )
            .await;
        *state.server.lock().unwrap() = Some(proc_);
        translator = Some(Arc::new(tr));
    }

    // 2. 管線
    let mut cfg = PipelineConfig::default();
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
    Ok(())
}

#[tauri::command]
pub async fn stop_session(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let handle = state.pipeline.lock().unwrap().take();
    if let Some(h) = handle {
        h.stop().await;
    }
    if let Some(f) = state.forwarder.lock().unwrap().take() {
        f.abort();
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

/// 解鎖/鎖定懸浮字幕(鎖定 = 滑鼠穿透)。
#[tauri::command]
pub fn set_overlay_interactive(app: AppHandle, interactive: bool) -> Result<(), String> {
    let w = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    w.set_ignore_cursor_events(!interactive)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("overlay-interactive", interactive);
    Ok(())
}

/// 顯示模式:bilingual / translation。
#[tauri::command]
pub fn set_overlay_mode(app: AppHandle, mode: String) -> Result<(), String> {
    app.emit("overlay-mode", mode).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_overlay(app: AppHandle, visible: bool) -> Result<(), String> {
    let w = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    if visible {
        w.show().map_err(|e| e.to_string())
    } else {
        w.hide().map_err(|e| e.to_string())
    }
}
