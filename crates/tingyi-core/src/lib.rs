//! 聽譯(tingyi)核心函式庫。
//!
//! 資料流:`audio::capture`(WASAPI loopback)→ `audio::resample`(16 kHz mono)
//! → `vad`(Silero 切段)→ `asr`(SenseVoice)→ `mt`(Hy-MT2 via llama-server)
//! → `postprocess`(台灣用語)→ 由 `pipeline` 統一發出事件。

pub mod audio;
pub mod vad;
pub mod endpoint;
pub mod sentence;
pub mod tuner;
pub mod speaker;
pub mod sysmon;
pub mod asr;
pub mod route;
pub mod cleanup;
pub mod mt;
pub mod postprocess;
pub mod metrics;
pub mod pipeline;
pub mod refine;
pub mod export;
pub mod eval;
pub mod model_download;
pub mod paths;
pub mod llama;
pub mod gpu;
