//! CLI agent bridge(選用):用系統的 node 執行打包進 resources 的 `bridge.mjs`。
//! 會用 codex/claude CLI 的人一定裝了 Node,所以不另外內建 Node runtime。

use crate::job::Job;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const BRIDGE_PORT: u16 = 47821;
pub const EXTENSION_ORIGIN: &str = "chrome-extension://jamiahhkhimbjajeddmncinemlmfkfhn";

/// 啟用哪些 CLI 後端(對應 bridge config.json 的 providers)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Providers {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

impl Default for Providers {
    /// 提示注入閘門只有 codex 實測通過,其餘預設關(與 bridge/src/main.ts 一致)
    fn default() -> Self {
        Self { claude: false, codex: true, gemini: false }
    }
}

/// 產生要寫入的 bridge config.json 內容。
/// - existing 有內容:保留 token、port 與其他欄位,只改 providers,並確保 allowedOrigins 含本擴充
/// - existing 為 None 或無法解析:新建(port 47821、隨機 token、allowedOrigins、openai: [])
pub fn merge_config(existing: Option<&str>, providers: Providers) -> Result<String> {
    let mut v = existing
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .filter(|v| v.get("token").and_then(|t| t.as_str()).is_some_and(|t| !t.is_empty()))
        .unwrap_or_else(|| {
            let bytes: [u8; 24] = rand::random();
            serde_json::json!({
                "port": BRIDGE_PORT,
                "token": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                "allowedOrigins": [],
                "openai": [],
            })
        });
    let obj = v.as_object_mut().ok_or_else(|| anyhow!("bridge 設定不是物件"))?;
    obj.insert("providers".into(), serde_json::to_value(providers)?);
    let origins = obj.entry("allowedOrigins").or_insert_with(|| serde_json::json!([]));
    if !origins.is_array() {
        *origins = serde_json::json!([]);
    }
    if let Some(list) = origins.as_array_mut() {
        if !list.iter().any(|o| o == EXTENSION_ORIGIN) {
            list.push(EXTENSION_ORIGIN.into());
        }
    }
    Ok(serde_json::to_string_pretty(&v)? + "\n")
}

/// 從 config.json 讀出 (port, token)
pub fn read_endpoint(config: &str) -> Option<(u16, String)> {
    let v: serde_json::Value = serde_json::from_str(config).ok()?;
    Some((v.get("port")?.as_u64()? as u16, v.get("token")?.as_str()?.to_string()))
}

/// 依 PATH 找 node.exe
pub fn find_node() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("node.exe")).find(|p| p.is_file())
}

pub struct BridgeProcess {
    child: Child,
}

impl BridgeProcess {
    pub fn spawn(node: &Path, script: &Path, log: &Path, job: &Job) -> Result<Self> {
        if !script.is_file() {
            return Err(anyhow!("找不到 bridge:{}", script.display()));
        }
        let f = std::fs::File::create(log).with_context(|| format!("建立 log {}", log.display()))?;
        let f2 = f.try_clone()?;
        let mut cmd = Command::new(node);
        cmd.arg(script).stdin(Stdio::null()).stdout(Stdio::from(f)).stderr(Stdio::from(f2));
        crate::job::hide_console(&mut cmd);
        let child = cmd.spawn().context("spawn bridge")?;
        if let Err(e) = job.assign(&child) {
            tracing::warn!("bridge 放入 Job Object 失敗:{e}");
        }
        Ok(Self { child })
    }

    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}
