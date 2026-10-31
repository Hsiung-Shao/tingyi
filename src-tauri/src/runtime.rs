//! `runtime.json`:本機程式目前狀態。程式寫、native host 讀。

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// 本地模型的狀態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlamaStatus {
    /// llama.cpp 或模型還沒下載
    NotDownloaded,
    Downloading,
    /// 已下載、沒在跑
    Stopped,
    Starting,
    Ready,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaInfo {
    pub port: u16,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInfo {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    /// 本機程式的 pid;native host 用它判斷程式還在不在
    pub pid: u32,
    pub version: String,
    pub status: LlamaStatus,
    /// 只有 status = Ready 時才有
    pub llama: Option<LlamaInfo>,
    /// bridge 有啟用且在跑時才有
    pub bridge: Option<BridgeInfo>,
    /// 錯誤或進度說明(給擴充 popup 顯示)
    pub message: Option<String>,
}

/// 先寫同目錄暫存檔再改名,讀的一方不會讀到半份檔
pub fn write_atomic(path: &Path, rt: &Runtime) -> io::Result<()> {
    let body = serde_json::to_vec_pretty(rt).map_err(io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, body)?;
    // Windows 的 rename 會取代既有檔案;失敗時不留暫存檔
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// 讀不到或格式錯回 None
pub fn read(path: &Path) -> Option<Runtime> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
