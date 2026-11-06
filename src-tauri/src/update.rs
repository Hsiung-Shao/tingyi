//! 自動更新:查 GitHub Release 的 latest.json、下載並驗證簽章、用 NSIS 安裝(tauri-plugin-updater)。
//! 測試用環境變數:`TINGYI_UPDATE_ENDPOINT` 換檢查網址、`TINGYI_UPDATE_DRY_RUN=1` 只下載驗證不安裝
//! (實跑不能蓋掉使用者已安裝的聽譯)。規則判斷在 `tingyi_core::update`。

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tingyi_core::update::{endpoint_override, should_notify};

use crate::state::AppState;

/// 查到的新版;`install_update` 用它下載安裝。
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    pub notes: String,
    pub date: Option<String>,
    /// 該不該主動提示(使用者略過這版時為 false;執行中由前端另外判斷;設定頁手動檢查照樣顯示)
    pub notify: bool,
}

#[derive(Serialize, Clone)]
struct Progress {
    done: u64,
    total: Option<u64>,
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 檢查有沒有新版;沒有回 None。`dismissed` 是使用者按過「略過這版」的版本。
#[tauri::command]
pub async fn check_update(app: AppHandle, dismissed: Option<String>) -> Result<Option<UpdateInfo>, String> {
    // 安裝時外掛直接 process::exit,不經 RunEvent::Exit:先做同一套收尾(停 whisper-server、放掉翻譯伺服器、刪 runtime 檔)
    let app_exit = app.clone();
    let mut builder = app.updater_builder().on_before_exit(move || {
        app_exit.state::<AppState>().shutdown_blocking();
        crate::web::remove_runtime_file();
    });
    if let Some(url) = endpoint_override(std::env::var("TINGYI_UPDATE_ENDPOINT").ok().as_deref()) {
        let url = tauri::Url::parse(&url).map_err(|e| e.to_string())?;
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    let update = builder.build().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let pending = app.state::<PendingUpdate>();
    let Some(update) = update else {
        *pending.0.lock().unwrap() = None;
        return Ok(None);
    };
    // 執行中不打擾由前端依當下狀態判斷(這裡算的話,檢查當下在執行中就會一直不提示)
    let info = UpdateInfo {
        notify: should_notify(false, dismissed.as_deref(), &update.version),
        version: update.version.clone(),
        current: update.current_version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|d| d.to_string()),
    };
    tracing::info!("有新版 {}(目前 {})", info.version, info.current);
    *pending.0.lock().unwrap() = Some(update);
    Ok(Some(info))
}

/// 下載(驗證簽章)並安裝;Windows 上安裝程式會結束聽譯,裝完自動重開。
/// 回傳 "dry-run-verified" 表示測試模式只驗證沒安裝。
#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    // 只複製不取走:下載或停回合失敗時,使用者可以直接再按一次
    let update = app.state::<PendingUpdate>().0.lock().unwrap().clone().ok_or("請先檢查更新")?;
    // 安裝會結束程式:先停回合(子程序另由 Job Object 跟著結束)
    if state.pipeline.lock().unwrap().is_some() {
        crate::commands::stop_inner(app.clone(), &state).await?;
    }
    let app2 = app.clone();
    let mut done = 0u64;
    let bytes = update
        .download(
            move |chunk, total| {
                done += chunk as u64;
                let _ = app2.emit("update-progress", Progress { done, total });
            },
            || {},
        )
        .await
        .map_err(|e| format!("下載或簽章驗證失敗:{e}"))?;
    tracing::info!("更新 {} 下載完成並通過簽章驗證({} bytes)", update.version, bytes.len());
    if std::env::var("TINGYI_UPDATE_DRY_RUN").ok().as_deref() == Some("1") {
        return Ok("dry-run-verified".into());
    }
    update.install(bytes).map_err(|e| format!("安裝失敗:{e}"))?;
    Ok("installing".into())
}
