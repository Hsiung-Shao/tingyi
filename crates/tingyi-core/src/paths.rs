//! 模型與執行檔的存放位置。
//!
//! 解析順序:
//! 1. 環境變數 `TINGYI_DATA_DIR`
//! 2. 開發用:workspace 根目錄下的 `data/`(存在才採用;已 gitignore)
//! 3. `%LOCALAPPDATA%\tingyi`
//!
//! 注意:在 MSIX 封裝的程式(例如 Claude 桌面版)內寫 `%LOCALAPPDATA%` 會被重導到
//! `Packages\<app>\LocalCache\Local`,外部程式看不到;開發期因此改用專案內 `data/`。

use std::path::PathBuf;

/// 編譯期固定的 workspace 根目錄(`crates/tingyi-core` 的上兩層)。
fn workspace_root() -> PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| manifest.join("..").join(".."))
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("TINGYI_DATA_DIR") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let dev = workspace_root().join("data");
    if dev.is_dir() {
        return dev;
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("tingyi")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

/// SenseVoice int8 模型目錄。
///
/// 用 2024-07-17 原版。2025-09-09 版是從 ASLP-lab WSYue-ASR(粵語微調)轉出,
/// 日、韓辨識會掉假名與諺文,不適合本專案。
pub const SENSEVOICE_DIR_NAME: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";

pub fn sensevoice_dir() -> PathBuf {
    models_dir().join(SENSEVOICE_DIR_NAME)
}

pub fn sensevoice_model() -> PathBuf {
    sensevoice_dir().join("model.int8.onnx")
}

pub fn sensevoice_tokens() -> PathBuf {
    sensevoice_dir().join("tokens.txt")
}

pub fn silero_vad_model() -> PathBuf {
    models_dir().join("silero_vad.onnx")
}

/// 翻譯模型 GGUF;`name` 例如 `Hy-MT2-1.8B-Q8_0`。
pub fn gguf_model(name: &str) -> PathBuf {
    models_dir().join(format!("{name}.gguf"))
}

/// 已知的翻譯模型選項(名稱、推估顯示記憶體)。
pub const KNOWN_MT_MODELS: &[(&str, &str)] = &[
    ("Hy-MT2-1.8B-Q8_0", "約 2.5 GB,預設"),
    ("Hy-MT2-1.8B-Q4_K_M", "約 1.5 GB,輕量"),
    ("Hy-MT2-7B-Q4_K_M", "約 5.5 GB,品質優先"),
];

/// llama-server 執行檔(解壓後放在 `bin/llama/`)。
pub fn llama_server_exe() -> PathBuf {
    bin_dir().join("llama").join("llama-server.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        std::env::set_var("TINGYI_DATA_DIR", "X:/somewhere");
        assert_eq!(data_dir(), PathBuf::from("X:/somewhere"));
        assert_eq!(gguf_model("a"), PathBuf::from("X:/somewhere/models/a.gguf"));
        std::env::remove_var("TINGYI_DATA_DIR");
    }
}
