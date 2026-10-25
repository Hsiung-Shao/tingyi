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

/// 日文專用模型(ReazonSpeech k2 v2:Zipformer RNN-T、159M 參數,sherpa-onnx 轉出)。
/// - ja-en 2025-01-17:官方日英雙語版(reazon-research/reazonspeech-k2-v2-ja-en)
/// - ja 2024-08-01:官方日文版(reazon-research/reazonspeech-k2-v2,Apache-2.0)
pub const REAZONSPEECH_JA_EN_DIR_NAME: &str = "sherpa-onnx-zipformer-ja-en-reazonspeech-2025-01-17";
pub const REAZONSPEECH_JA_DIR_NAME: &str = "sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01";

/// 英文專用模型(NVIDIA Parakeet TDT 0.6B v2,只有英文,CC-BY-4.0)。v3 多語版英文 WER 較差(hayamimi 實測)。
pub const PARAKEET_DIR_NAME: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8";

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
    ("Sakura-7B-Qwen2.5-v1.0-IQ4_XS", "約 4.3 GB,日→中特化(僅日文)"),
    ("Sakura-1.5B-Qwen2.5-v1.0-F16", "約 3.6 GB,日→中特化小模型(僅日文)"),
    ("Seed-X-PPO-7B-Q4_K_M", "約 4.6 GB,ByteDance 28 語翻譯專用"),
    ("LMT-60-4B-Q4_K_M", "約 2.7 GB,NiuTrans 中英為核心 60 語(Apache-2.0)"),
    ("LMT-60-8B-Q4_K_M", "約 5.0 GB,NiuTrans 中英為核心 60 語(Apache-2.0)"),
    ("TranslateGemma-4B-it-Q4_K_M", "約 2.5 GB,Google 55 語翻譯專用"),
    ("TranslateGemma-12B-it-Q4_K_M", "約 7.3 GB,Google 55 語翻譯專用,品質優先"),
];

/// llama-server 執行檔(解壓後放在 `bin/llama/`)。
pub fn llama_server_exe() -> PathBuf {
    bin_dir().join("llama").join("llama-server.exe")
}

/// whisper-server 執行檔(`scripts/build-whisper.ps1` 產出,放在 `bin/whisper/`)。
pub fn whisper_server_exe() -> PathBuf {
    bin_dir().join("whisper").join("whisper-server.exe")
}

/// 說話者 embedding 模型(3D-Speaker CAM++ 中英 advanced,16 kHz)。
pub fn speaker_model() -> PathBuf {
    models_dir().join("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx")
}

/// Whisper large-v3-turbo q8_0 GGML 模型。
pub fn whisper_model() -> PathBuf {
    models_dir().join("ggml-large-v3-turbo-q8_0.bin")
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
