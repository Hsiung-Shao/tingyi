//! 聽譯 Tauri 後端:管理共用翻譯伺服器與辨識翻譯管線,把事件轉發給前端視窗;
//! 也兼任瀏覽器擴充的 native messaging host(見 `web.rs`)。

mod commands;
pub mod listen;
pub mod native_host;
pub mod register;
pub mod runtime;
pub mod settings;
mod state;
mod web;

pub use commands::StartOptions;
pub use web::run_native_host;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tingyi_core::translate_server::{close_action, CloseAction};

/// 擴充喚起:在背景啟動,不顯示主視窗。
const ARG_BACKGROUND: &str = "--background";
/// 擴充喚起:讓網頁翻譯持有共用翻譯伺服器。
const ARG_SERVE_WEB: &str = "--serve-web";

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 啟動參數或單一實例轉交來的參數:`--serve-web` 讓網頁翻譯取得伺服器;沒有 `--background` 才顯示主視窗。
fn handle_args(app: &AppHandle, args: &[String]) {
    if args.iter().any(|a| a == ARG_SERVE_WEB) {
        web::serve_web(app);
    }
    if !args.iter().any(|a| a == ARG_BACKGROUND) {
        show_main(app);
    }
}

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
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| handle_args(app, &args)))
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
            commands::set_subtitle_hide,
            commands::show_overlay,
            commands::export_transcript,
            commands::list_audio_apps,
            commands::set_audio_source,
            commands::get_audio_source,
            commands::routed_models_status,
            commands::translate_packages_status,
            commands::download_model,
            commands::cancel_model_download,
            commands::web_status,
            commands::web_start,
            commands::web_stop,
            commands::set_web_model,
            commands::get_prefs,
            commands::save_prefs,
        ])
        .setup(|app| {
            // 懸浮字幕視窗預設滑鼠穿透。
            if let Some(w) = app.get_webview_window("overlay") {
                let _ = w.set_ignore_cursor_events(true);
            }
            web::init(app.handle());
            web::start_watch(app.handle());
            // 開發版預設不寫註冊,避免把安裝版的 native host 搶到開發用 exe
            let flag = std::env::var("TINGYI_REGISTER_HOST").ok();
            if register::should_register(cfg!(debug_assertions), flag.as_deref()) {
                if let Ok(exe) = std::env::current_exe() {
                    if let Err(e) = register::register(&exe, &web::native_host_manifest()) {
                        tracing::warn!("登錄 native messaging host 失敗:{e}");
                    }
                }
            }

            let open = MenuItem::with_id(app, "open", "開啟主視窗", true, None::<&str>)?;
            let stop_web = MenuItem::with_id(app, "stop-web", "停止網頁翻譯", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &stop_web, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().ok_or("缺少程式圖示")?)
                .tooltip("聽譯")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "stop-web" => web::stop_web(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            let args: Vec<String> = std::env::args().collect();
            handle_args(app.handle(), &args);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            match event {
                // 網頁翻譯還在用共用伺服器 → 只縮到系統匣;否則照原本行為結束整個程式
                WindowEvent::CloseRequested { api, .. } => {
                    let st = window.state::<state::AppState>();
                    if close_action(st.web.serving()) == CloseAction::HideToTray {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                WindowEvent::Destroyed => window.app_handle().exit(0),
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<state::AppState>().shutdown_blocking();
                web::remove_runtime_file();
            }
        });
}
