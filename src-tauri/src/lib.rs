//! 聽譯 Tauri 後端:管理 llama-server 與辨識翻譯管線,把事件轉發給前端視窗。

mod commands;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tingyi_core=debug".into()),
        )
        .with_target(false)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_info,
            commands::start_session,
            commands::stop_session,
            commands::get_metrics,
            commands::export_diagnostics,
            commands::set_segmentation,
            commands::rename_speaker,
            commands::list_speakers,
            commands::set_overlay_interactive,
            commands::set_overlay_mode,
            commands::show_overlay,
            commands::export_transcript,
            commands::list_audio_apps,
            commands::set_audio_source,
            commands::get_audio_source,
            commands::routed_models_status,
            commands::download_model,
            commands::cancel_model_download,
        ])
        .setup(|app| {
            // 懸浮字幕視窗預設滑鼠穿透。
            if let Some(w) = app.get_webview_window("overlay") {
                let _ = w.set_ignore_cursor_events(true);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 主視窗關閉 → 結束整個程式(含 llama-server)。
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() == "main" {
                    let st = window.state::<state::AppState>();
                    st.shutdown_blocking();
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
