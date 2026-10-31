//! 向 Chrome/Edge/Chromium 登錄 native messaging host(HKCU,不需系統管理員)。
//! 程式每次啟動都重寫一次,安裝位置變了也會跟上。

use crate::native_host::HOST_NAME;
use serde_json::{json, Value};
use std::io;
use std::path::Path;

/// 允許呼叫本程式的擴充 ID(開發版;上架商店後加上商店版 ID)
pub const EXTENSION_IDS: &[&str] = &["jamiahhkhimbjajeddmncinemlmfkfhn"];

/// 程式啟動時要不要寫 native host 註冊:`debug_build` 為開發版,`env_flag` 是 `TINGYI_REGISTER_HOST` 的值。
pub fn should_register(debug_build: bool, env_flag: Option<&str>) -> bool {
    !debug_build || env_flag == Some("1")
}

/// (瀏覽器名稱, HKCU 底下放 NativeMessagingHosts 的位置)
const BROWSERS: [(&str, &str); 3] = [
    ("Chrome", r"Software\Google\Chrome"),
    ("Edge", r"Software\Microsoft\Edge"),
    ("Chromium", r"Software\Chromium"),
];

fn host_key(base: &str) -> String {
    format!(r"{base}\NativeMessagingHosts\{HOST_NAME}")
}

/// native-host.json 的內容:name、description、path(本程式 exe 絕對路徑)、type=stdio、
/// allowed_origins(每個 ID 一筆 `chrome-extension://<id>/`)
pub fn host_manifest(exe: &Path) -> Value {
    json!({
        "name": HOST_NAME,
        "description": "聽譯:提供瀏覽器網頁翻譯用的本地翻譯模型連線資訊",
        "path": exe.display().to_string(),
        "type": "stdio",
        "allowed_origins": EXTENSION_IDS.iter().map(|id| format!("chrome-extension://{id}/")).collect::<Vec<_>>(),
    })
}

/// 要寫的 HKCU 機碼(相對 HKEY_CURRENT_USER)與預設值(native-host.json 的絕對路徑)
pub fn registry_entries(manifest_path: &Path) -> Vec<(String, String)> {
    BROWSERS.iter().map(|(_, base)| (host_key(base), manifest_path.display().to_string())).collect()
}

/// 寫 native-host.json 並寫入所有機碼
pub fn register(exe: &Path, manifest_path: &Path) -> io::Result<()> {
    if let Some(dir) = manifest_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec_pretty(&host_manifest(exe)).map_err(io::Error::other)?;
    std::fs::write(manifest_path, body)?;
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    for (key, value) in registry_entries(manifest_path) {
        let (k, _) = hkcu.create_subkey(&key)?;
        k.set_value("", &value)?;
    }
    Ok(())
}

/// 每個瀏覽器(Chrome、Edge)的機碼是否指向這份 manifest;Chromium 只供測試,不列
pub fn registered_browsers(manifest_path: &Path) -> Vec<(&'static str, bool)> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let want = manifest_path.display().to_string();
    BROWSERS
        .iter()
        .filter(|(name, _)| *name != "Chromium")
        .map(|(name, base)| {
            let ok = hkcu
                .open_subkey(host_key(base))
                .and_then(|k| k.get_value::<String, _>(""))
                .is_ok_and(|v| v.eq_ignore_ascii_case(&want));
            (*name, ok)
        })
        .collect()
}

/// 解除安裝時移除機碼
pub fn unregister() -> io::Result<()> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    for (_, base) in BROWSERS {
        match hkcu.delete_subkey_all(host_key(base)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}
