//! 本機程式的檔案位置。
//!
//! 資料目錄解析順序:
//! 1. 環境變數 `ATX_DATA_DIR`
//! 2. 開發用(只限 debug 版):`app/data/`(存在才採用;已 gitignore)
//! 3. `%LOCALAPPDATA%\agent-translator`
//!
//! Claude 桌面版(MSIX)底下啟動的行程寫 `%LOCALAPPDATA%` 新資料夾會被重導到套件私有位置,
//! 開發期因此改用專案內 `data/`(見 error_claude_msix_appdata_virtualization)。

use std::path::{Path, PathBuf};

/// 編譯期固定的 `app/` 目錄(`src-tauri` 的上一層)。
fn app_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(Path::to_path_buf).unwrap_or_else(|| manifest.join(".."))
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ATX_DATA_DIR") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    // 只有 debug 版看 app/data:正式版在開發機上編譯也不能被編譯期路徑帶走
    let dev = app_root().join("data");
    if cfg!(debug_assertions) && dev.is_dir() {
        return dev;
    }
    let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    base.join("agent-translator")
}

/// llama.cpp 執行檔目錄(解壓後的 zip 內容)
pub fn llama_dir(data: &Path) -> PathBuf {
    data.join("bin").join("llama")
}

pub fn llama_server_exe(data: &Path) -> PathBuf {
    llama_dir(data).join("llama-server.exe")
}

pub fn models_dir(data: &Path) -> PathBuf {
    data.join("models")
}

pub fn logs_dir(data: &Path) -> PathBuf {
    data.join("logs")
}

/// 本機程式目前狀態,native host 讀它回覆擴充
pub fn runtime_file(data: &Path) -> PathBuf {
    data.join("runtime.json")
}

/// 擴充最近一次連線的時間(native host 寫,程式介面讀)
pub fn hello_file(data: &Path) -> PathBuf {
    data.join("last-hello.json")
}

/// Chrome/Edge 讀的 native messaging host 設定檔
pub fn native_host_manifest(data: &Path) -> PathBuf {
    data.join("native-host.json")
}

/// 程式自己的設定(bridge 開關等)
pub fn app_settings_file(data: &Path) -> PathBuf {
    data.join("settings.json")
}

/// bridge 的設定檔沿用 bridge 既有位置,不搬(既有 token 才不會失效)
pub fn bridge_config_file() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::var("HOME").map(PathBuf::from).unwrap_or_default().join(".config"));
    base.join("agent-translator").join("config.json")
}
