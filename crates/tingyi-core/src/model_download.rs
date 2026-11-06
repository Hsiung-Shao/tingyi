//! 專用辨識模型的 app 內下載:Hugging Face 固定 commit 逐檔下載、續傳、SHA256 校驗、暫存後提交。
//!
//! - 每檔先下載到 `models/.download/<dir>/<name>.part`;寫完整檔校驗大小與 SHA256,通過才改名去掉
//!   `.part`(改名 = 已校驗的提交點,重啟後可直接信任)。
//! - 全部檔案提交後才放進正式目錄 `models/<dir>`:目錄不存在就整個改名過去,已存在就逐檔移入,
//!   不刪使用者自己放的檔案。半途中斷時正式目錄不會出現半套。
//! - 同步實作,跑在 std 執行緒;取消在每次讀取後檢查。

use crate::asr::Lang;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 模型裡的一個檔案(大小與 SHA256 固定,換版本要一起改)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSpec {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// 一個可下載的專用模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: &'static str,
    pub lang: Lang,
    pub label: &'static str,
    pub dir_name: &'static str,
    pub repo: &'static str,
    pub commit: &'static str,
    pub license: &'static str,
    pub license_url: &'static str,
    pub files: &'static [FileSpec],
    /// 不在 Hugging Face 的套件(例如 GitHub Releases):檔案網址為 `{url_base}/{name}`。
    pub url_base: Option<&'static str>,
    /// 安裝後把 .zip 解壓到套件目錄。
    pub unzip: bool,
    /// 就緒還要有這個檔(例如解壓出來的 `llama-server.exe`)。
    pub ready_marker: Option<&'static str>,
}

impl ModelSpec {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// `{base}/{repo}/resolve/{commit}/{name}`;每次都從這個原始網址開始(HF 會 307 到會過期的 CDN 網址)。
    /// 有 `url_base` 的套件(GitHub Releases)固定用 `{url_base}/{name}`。
    pub fn url(&self, base: &str, file: &FileSpec) -> String {
        match self.url_base {
            Some(u) => format!("{}/{}", u.trim_end_matches('/'), file.name),
            None => format!("{}/{}/resolve/{}/{}", base.trim_end_matches('/'), self.repo, self.commit, file.name),
        }
    }
}

/// 把 zip 解壓到 dest(zip 內的子目錄照原樣建立,拒絕跳出 dest 的路徑;移植自 agent-translator)。
pub fn extract_zip(zip_path: &Path, dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?).map_err(io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        // enclosed_name 拒絕絕對路徑與 `..`(zip slip)
        let Some(rel) = entry.enclosed_name() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("zip 內有不安全的路徑:{}", entry.name())));
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create(&out)?)?;
    }
    Ok(())
}

pub const DEFAULT_BASE_URL: &str = "https://huggingface.co";

/// 日文 ReazonSpeech k2 v2(與 sherpa-onnx 2024-08-01 壓縮包內的 int8 檔 SHA256 相同)。
pub const REAZONSPEECH: ModelSpec = ModelSpec {
    id: "reazonspeech",
    lang: Lang::Ja,
    label: "日文 ReazonSpeech k2 v2",
    dir_name: crate::paths::REAZONSPEECH_JA_DIR_NAME,
    repo: "reazon-research/reazonspeech-k2-v2",
    commit: "291488c8151be24d7da4bf7af26e533fad96e407",
    license: "Apache-2.0(reazon-research)",
    license_url: "https://huggingface.co/reazon-research/reazonspeech-k2-v2",
    files: &[
        FileSpec {
            name: "encoder-epoch-99-avg-1.int8.onnx",
            size: 154_670_139,
            sha256: "2c7bd08a8a99f9ddd0d9e458456577b1f6279214e51426f114f9eced44c54e1d",
        },
        FileSpec {
            name: "decoder-epoch-99-avg-1.int8.onnx",
            size: 2_959_337,
            sha256: "8f0bff94d38797b03b762634ed03211a8e303d06cc4603cdd0cf4199d6eb1485",
        },
        FileSpec {
            name: "joiner-epoch-99-avg-1.int8.onnx",
            size: 2_696_970,
            sha256: "49cc7ea1d3d35a40a27442db5e89996da64bf0e683a903dce76e99e57a12e4de",
        },
        FileSpec {
            name: "tokens.txt",
            size: 45_754,
            sha256: "2c3ac659818a48a0c04010e0593bbc4d7c8a24a054340b01131499c05fd52def",
        },
    ],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// 英文 Parakeet TDT 0.6B v2 int8(與 sherpa-onnx 壓縮包內檔案 SHA256 相同)。
pub const PARAKEET: ModelSpec = ModelSpec {
    id: "parakeet",
    lang: Lang::En,
    label: "英文 Parakeet TDT 0.6B v2",
    dir_name: crate::paths::PARAKEET_DIR_NAME,
    repo: "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
    commit: "1ab9323565ddb038682214b292f588070a538ce2",
    license: "CC-BY-4.0(NVIDIA parakeet-tdt-0.6b-v2,經 sherpa-onnx 轉為 int8)",
    license_url: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2",
    files: &[
        FileSpec {
            name: "encoder.int8.onnx",
            size: 652_184_296,
            sha256: "a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab",
        },
        FileSpec {
            name: "decoder.int8.onnx",
            size: 7_257_753,
            sha256: "b6bb64963457237b900e496ee9994b59294526439fbcc1fecf705b31a15c6b4e",
        },
        FileSpec {
            name: "joiner.int8.onnx",
            size: 1_739_080,
            sha256: "7946164367946e7f9f29a122407c3252b680dbae9a51343eb2488d057c3c43d2",
        },
        FileSpec {
            name: "tokens.txt",
            size: 9_384,
            sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
        },
    ],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

pub const MODELS: &[ModelSpec] = &[REAZONSPEECH, PARAKEET];

/// llama.cpp b11405 預編譯 Vulkan 版(NVIDIA/AMD/Intel 都能用),安裝到 `bin/llama`;與 `scripts/download-models.ps1` 相同。
pub const LLAMA_CPP: ModelSpec = ModelSpec {
    id: "llama-cpp",
    lang: Lang::Unknown,
    label: "llama.cpp b11405(Vulkan)",
    dir_name: "llama",
    repo: "ggml-org/llama.cpp",
    commit: "b11405",
    license: "MIT(ggml-org/llama.cpp)",
    license_url: "https://github.com/ggml-org/llama.cpp",
    files: &[FileSpec {
        name: "llama-b11405-bin-win-vulkan-x64.zip",
        size: 33_308_372,
        sha256: "0f3764246cc4c365153dc33a24448921526d91bba5ee3195308fa692d2031884",
    }],
    url_base: Some("https://github.com/ggml-org/llama.cpp/releases/download/b11405"),
    unzip: true,
    ready_marker: Some("llama-server.exe"),
};

/// Hy-MT2 1.8B Q8_0 翻譯模型,安裝到 `models/hy-mt2-1.8b`;與 `scripts/download-models.ps1` 相同。
pub const HY_MT2_1_8B: ModelSpec = ModelSpec {
    id: "hy-mt2-1.8b",
    lang: Lang::Unknown,
    label: "Hy-MT2 1.8B 翻譯模型",
    dir_name: "hy-mt2-1.8b",
    repo: "tencent/Hy-MT2-1.8B-GGUF",
    commit: "a0c709d9fac510f2c807aa3af52872340dc37a4a",
    license: "Apache-2.0(tencent/Hy-MT2)",
    license_url: "https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF",
    files: &[FileSpec {
        name: "Hy-MT2-1.8B-Q8_0.gguf",
        size: 1_908_528_192,
        sha256: "5c3fe0b1408a5ceb0143184ef247b11b579c525f4b02b060e6c851bb76fef1a4",
    }],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// 翻譯用的套件:llama.cpp 裝在 `bin_dir()`,翻譯模型裝在 `models_dir()`。
pub const TRANSLATE_PACKAGES: &[ModelSpec] = &[LLAMA_CPP, HY_MT2_1_8B];

/// SenseVoice-Small int8(2024-07-17 原版);辨識與即時預覽都靠它,必要。與 `scripts/download-models.ps1` 相同。
pub const SENSEVOICE: ModelSpec = ModelSpec {
    id: "sensevoice",
    lang: Lang::Unknown,
    label: "SenseVoice-Small 辨識模型(必要)",
    dir_name: crate::paths::SENSEVOICE_DIR_NAME,
    repo: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    commit: "2365baeacb507f821a0c8120fcee3d484dba7a07",
    license: "Apache-2.0(FunAudioLLM/SenseVoice,經 sherpa-onnx 轉出)",
    license_url: "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    files: &[
        FileSpec {
            name: "model.int8.onnx",
            size: 239_233_841,
            sha256: "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
        },
        FileSpec {
            name: "tokens.txt",
            size: 315_894,
            sha256: "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc",
        },
    ],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// Silero VAD(切段用),必要;安裝到 `models/silero-vad`。與 `scripts/download-models.ps1` 相同。
pub const SILERO_VAD: ModelSpec = ModelSpec {
    id: "silero-vad",
    lang: Lang::Unknown,
    label: "Silero VAD 語音偵測(必要)",
    dir_name: "silero-vad",
    repo: "k2-fsa/sherpa-onnx",
    commit: "asr-models",
    license: "MIT(snakers4/silero-vad)",
    license_url: "https://github.com/snakers4/silero-vad",
    files: &[FileSpec {
        name: "silero_vad.onnx",
        size: 643_854,
        sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
    }],
    url_base: Some("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"),
    unzip: false,
    ready_marker: None,
};

/// 3D-Speaker CAM++ 說話者辨識模型,選用(沒有就不分說話者);安裝到 `models/speaker`。與 `scripts/download-models.ps1` 相同。
pub const SPEAKER: ModelSpec = ModelSpec {
    id: "speaker",
    lang: Lang::Unknown,
    label: "說話者辨識 CAM++(選用)",
    dir_name: "speaker",
    repo: "csukuangfj/speaker-embedding-models",
    commit: "0743f301363dec56491a490f6d6cbc9d67f9a3bf",
    license: "Apache-2.0(modelscope 3D-Speaker)",
    license_url: "https://huggingface.co/csukuangfj/speaker-embedding-models",
    files: &[FileSpec {
        name: "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx",
        size: 28_281_164,
        sha256: "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2",
    }],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// 辨識基本模型:安裝版第一次使用前必須下載(SenseVoice、VAD),說話者模型選用。
pub const BASE_PACKAGES: &[ModelSpec] = &[SENSEVOICE, SILERO_VAD, SPEAKER];

/// whisper.cpp 官方預編譯 CUDA 12.4 版(含 whisper-server 與 CUDA DLL);解壓到 `bin/whisper/Release/`。
/// RTX 50 系列沒有原生程式碼,第一次辨識要現場編譯約 20–30 秒(之後有 CUDA 快取)。
pub const WHISPER_SERVER: ModelSpec = ModelSpec {
    id: "whisper-server",
    lang: Lang::Unknown,
    label: "whisper.cpp b5454 伺服器(CUDA 12.4,需 NVIDIA 顯示卡)",
    dir_name: "whisper",
    repo: "ggml-org/whisper.cpp",
    commit: "b5454",
    license: "MIT(ggml-org/whisper.cpp)",
    license_url: "https://github.com/ggml-org/whisper.cpp",
    files: &[FileSpec {
        name: "whisper-bin-win-cuda-12.4.0-x64.zip",
        size: 684_913_404,
        sha256: "afef0b881c500958921c3f5523b50e59ee2ec9b6f5cbd25b324c51ed308a957a",
    }],
    url_base: Some("https://github.com/ggml-org/whisper.cpp/releases/download/b5454"),
    unzip: true,
    ready_marker: None,
};

/// Whisper large-v3-turbo q8_0;安裝到 `models/whisper/`(下載腳本放 models 根目錄,兩處都認)。
pub const WHISPER_TURBO: ModelSpec = ModelSpec {
    id: "whisper-turbo-model",
    lang: Lang::Unknown,
    label: "Whisper large-v3-turbo q8_0 模型",
    dir_name: "whisper",
    repo: "ggerganov/whisper.cpp",
    commit: "5359861c739e955e79d9a303bcbc70fb988958b1",
    license: "MIT(openai/whisper)",
    license_url: "https://huggingface.co/ggerganov/whisper.cpp",
    files: &[FileSpec {
        name: "ggml-large-v3-turbo-q8_0.bin",
        size: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
    }],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// Qwen3-ASR 0.6B int8(sherpa-onnx 轉出,作者 csukuangfj 的 HF 鏡像)。
pub const QWEN3_06B: ModelSpec = ModelSpec {
    id: "qwen3-0.6b",
    lang: Lang::Unknown,
    label: "Qwen3-ASR 0.6B int8(實驗)",
    dir_name: "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
    repo: "csukuangfj2/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
    commit: "68818b2313fe77bd06f6a7c5068ff3ef59d02b8a",
    license: "Apache-2.0(Qwen/Qwen3-ASR)",
    license_url: "https://huggingface.co/Qwen/Qwen3-ASR-0.6B",
    files: &[
        FileSpec { name: "conv_frontend.onnx", size: 44_148_281, sha256: "d22dc4423e0940e49884e903d2ea2f7e5567c14fc1aed97e4e26d6b8f208ef9e" },
        FileSpec { name: "encoder.int8.onnx", size: 182_491_662, sha256: "60748d3e6744a57c9c91e1b17424a6c2990567e8adceb0783940c03ed98fa9d9" },
        FileSpec { name: "decoder.int8.onnx", size: 755_914_231, sha256: "4f6885be5959ae26af3089d38ee7972c5fafbeeb1cf8d5e76eab6d8b61ca5771" },
        FileSpec { name: "tokenizer/merges.txt", size: 1_671_853, sha256: "8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5" },
        FileSpec { name: "tokenizer/vocab.json", size: 2_776_833, sha256: "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910" },
        FileSpec { name: "tokenizer/tokenizer_config.json", size: 12_487, sha256: "4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c" },
    ],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// Qwen3-ASR 1.7B int8:sherpa-onnx 文件指向的 ModelScope 版(GitHub/HF 官方沒有),目錄照原樣保留 `model_1.7B/`。
pub const QWEN3_17B: ModelSpec = ModelSpec {
    id: "qwen3-1.7b",
    lang: Lang::Unknown,
    label: "Qwen3-ASR 1.7B int8(實驗,ModelScope)",
    dir_name: "qwen3-asr-onnx-modelscope",
    repo: "zengshuishui/Qwen3-ASR-onnx",
    commit: "master",
    license: "Apache-2.0(Qwen/Qwen3-ASR)",
    license_url: "https://modelscope.cn/models/zengshuishui/Qwen3-ASR-onnx",
    files: &[
        FileSpec { name: "model_1.7B/conv_frontend.onnx", size: 48_080_441, sha256: "fa894a4ba53da6a4238f2a6ca0b09362e505d39cecbd646051b033e2e8d7e2fb" },
        FileSpec { name: "model_1.7B/encoder.int8.onnx", size: 314_222_162, sha256: "436fbd910a0c8914851e5ac1354e807be9f283d08a5da728adaa609731c41469" },
        FileSpec { name: "model_1.7B/decoder.int8.onnx", size: 2_037_458_645, sha256: "c43c853fa6e97d08365cb8a5502b360b595cd43c00dc60e4d8ca7cc18cad460b" },
        FileSpec { name: "tokenizer/merges.txt", size: 1_671_853, sha256: "8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5" },
        FileSpec { name: "tokenizer/vocab.json", size: 2_776_833, sha256: "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910" },
        FileSpec { name: "tokenizer/tokenizer_config.json", size: 12_487, sha256: "4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c" },
    ],
    url_base: Some("https://modelscope.cn/models/zengshuishui/Qwen3-ASR-onnx/resolve/master"),
    unzip: false,
    ready_marker: None,
};

/// Cohere Transcribe 14 語 int8(sherpa-onnx 轉出,作者 csukuangfj 的 HF 鏡像)。
pub const COHERE: ModelSpec = ModelSpec {
    id: "cohere",
    lang: Lang::Unknown,
    label: "Cohere Transcribe int8(實驗,需指定語言)",
    dir_name: "sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01",
    repo: "csukuangfj2/sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01",
    commit: "156a470cf08eefe706a0004f3c52d9ee567ca7a0",
    license: "Apache-2.0(CohereLabs/cohere-transcribe-03-2026)",
    license_url: "https://huggingface.co/CohereLabs/cohere-transcribe-03-2026",
    files: &[
        FileSpec { name: "encoder.int8.onnx", size: 3_090_822, sha256: "cf704f8cfa90e3f0a76f9ffc05998bdf00ba9ae983192c14a85a3a5eb008b367" },
        FileSpec { name: "encoder.int8.onnx.data", size: 2_731_503_072, sha256: "bcf1b7148c8518ae52df1ad2d2fc2b4e89261ea23e6c874eef1d9f55bcbaa4a3" },
        FileSpec { name: "decoder.int8.onnx", size: 153_250_705, sha256: "8372ca6c8ff4db8b916ca3592f5c757a715e691b9edec751ba19b29fc854baf9" },
        FileSpec { name: "tokens.txt", size: 207_437, sha256: "013ede043ae2480e3a9205cc34550d9686100cc682bacc90f702facdfbb93035" },
    ],
    url_base: None,
    unzip: false,
    ready_marker: None,
};

/// 下拉選單引擎各自要下載的套件(SenseVoice 靠基本模型、專用模型有自己的卡)。
/// 壓縮檔全部解完才寫的標記檔(放在解壓目錄);解到一半被中斷時沒有它,就緒判斷才不會誤認。
pub const UNZIP_DONE_MARKER: &str = ".unzip-done";

pub const ENGINE_PACKAGES: &[ModelSpec] = &[WHISPER_SERVER, WHISPER_TURBO, QWEN3_06B, QWEN3_17B, COHERE];

pub fn engine_packages(engine: &str) -> Vec<&'static ModelSpec> {
    match engine {
        "whisper-turbo" => vec![&WHISPER_SERVER, &WHISPER_TURBO],
        "qwen3-0.6b" => vec![&QWEN3_06B],
        "qwen3-1.7b" => vec![&QWEN3_17B],
        "cohere" => vec![&COHERE],
        _ => Vec::new(),
    }
}

/// 依 id 找專用辨識模型、翻譯套件或辨識基本模型。
pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().chain(TRANSLATE_PACKAGES).chain(BASE_PACKAGES).chain(ENGINE_PACKAGES).find(|m| m.id == id)
}

pub fn spec_for_dir(dir_name: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.dir_name == dir_name)
}

/// 下載來源;環境變數 `TINGYI_MODEL_BASE_URL` 可覆寫(SHA256 固定,換來源也只會得到同一份檔案)。
pub fn base_url() -> String {
    std::env::var("TINGYI_MODEL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// 模型目錄的狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelStatus {
    /// 每個檔案都存在且大小正確。
    Ready,
    /// 目錄存在但缺檔或大小不對(列出問題檔名)。
    Partial { missing: Vec<String> },
    /// 目錄不存在。
    Absent,
}

fn file_len(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// 依規格檢查 `models_dir/<dir_name>`。
pub fn model_status(spec: &ModelSpec, models_dir: &Path) -> ModelStatus {
    let dir = models_dir.join(spec.dir_name);
    if !dir.is_dir() {
        return ModelStatus::Absent;
    }
    let mut missing: Vec<String> = spec
        .files
        .iter()
        .filter(|f| file_len(&dir.join(f.name)) != Some(f.size))
        .map(|f| f.name.to_string())
        .collect();
    if let Some(marker) = spec.ready_marker {
        if !dir.join(marker).is_file() {
            missing.push(marker.to_string());
        }
    }
    if missing.is_empty() {
        ModelStatus::Ready
    } else {
        ModelStatus::Partial { missing }
    }
}

/// 目錄是否可用:在規格表裡就用 `model_status`,不在(例如 bench 用的 ja-en 版本)退回只看目錄存在。
pub fn dir_ready(models_dir: &Path, dir_name: &str) -> bool {
    match spec_for_dir(dir_name) {
        Some(s) => model_status(s, models_dir) == ModelStatus::Ready,
        None => models_dir.join(dir_name).exists(),
    }
}

/// 小於這個大小的檔案一律從頭下載(不值得續傳,HF 也可能不經 CDN 直接回應)。
pub const SMALL_FILE: u64 = 1 << 20;

/// 單一檔案該怎麼處理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePlan {
    /// 從頭下載(沒有暫存檔、暫存檔是空的,或小檔)。
    Fresh,
    /// 從這個位元組接續(送 `Range: bytes=n-`)。
    Resume(u64),
    /// 暫存檔已經完整:不發請求,直接校驗。
    VerifyOnly,
    /// 暫存檔比應有的大:刪掉重下。
    Restart,
    /// 已提交(去掉 .part 且大小正確):略過。
    Skip,
}

/// `part_len`:暫存檔長度;`committed_len`:暫存目錄裡已提交檔案的長度。
pub fn plan_file(part_len: Option<u64>, committed_len: Option<u64>, size: u64) -> FilePlan {
    if committed_len == Some(size) {
        return FilePlan::Skip;
    }
    match part_len {
        None | Some(0) => FilePlan::Fresh,
        Some(n) if n == size => FilePlan::VerifyOnly,
        Some(n) if n > size => FilePlan::Restart,
        Some(_) if size < SMALL_FILE => FilePlan::Fresh,
        Some(n) => FilePlan::Resume(n),
    }
}

/// 解析 `Content-Range: bytes S-E/T`。
pub fn parse_content_range(v: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = v.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// 收到回應後怎麼做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseAction {
    /// 206 且起點正確:接在暫存檔後面寫。
    Append,
    /// 200(伺服器忽略 Range):截斷暫存檔從頭寫。
    Truncate,
    /// 範圍對不上(206 起點或總長錯、416):刪掉暫存檔重下。
    Restart,
    /// 暫時性錯誤(429、5xx):稍後以 Range 重試。
    Retry,
    /// 不可重試的錯誤(404 等)。
    Fail(String),
}

/// `from`:這次請求的起點(0 = 沒送 Range);`size`:檔案應有大小。
pub fn classify_response(
    status: u16,
    content_range: Option<&str>,
    content_length: Option<u64>,
    from: u64,
    size: u64,
) -> ResponseAction {
    match status {
        206 => match content_range.and_then(parse_content_range) {
            Some((start, end, total)) if start == from && total == size && end + 1 == size => ResponseAction::Append,
            _ => ResponseAction::Restart,
        },
        200 => match content_length {
            Some(len) if len != size => ResponseAction::Fail(format!("HTTP 200 但長度 {len} 與預期 {size} 不符")),
            _ => ResponseAction::Truncate,
        },
        416 => ResponseAction::Restart,
        429 | 500..=599 => ResponseAction::Retry,
        other => ResponseAction::Fail(format!("HTTP {other}")),
    }
}

/// 下載階段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Downloading,
    Verifying,
    Done,
    Error,
    Cancelled,
}

/// 推給前端的進度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub id: String,
    pub phase: Phase,
    /// 已下載位元組(整個模型合計)。
    pub done: u64,
    pub total: u64,
    /// 最近的下載速度(位元組/秒)。
    pub bps: u64,
    pub message: Option<String>,
}

/// 進度節流:最多每 `PROGRESS_INTERVAL` 一次,或進度變化 ≥ 1% 時;完成一律發。
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub struct ProgressThrottle {
    total: u64,
    last: Option<(Instant, u64)>,
}

impl ProgressThrottle {
    pub fn new(total: u64) -> Self {
        Self { total, last: None }
    }

    /// 這次要不要發;回 true 時記住這次的時間與進度。
    pub fn should_emit(&mut self, now: Instant, done: u64) -> bool {
        let emit = match self.last {
            None => true,
            Some(_) if done >= self.total => true,
            Some((t, d)) => {
                now.saturating_duration_since(t) >= PROGRESS_INTERVAL || done.saturating_sub(d) >= (self.total / 100).max(1)
            }
        };
        if emit {
            self.last = Some((now, done));
        }
        emit
    }
}

/// 把 I/O 錯誤轉成使用者看得懂的訊息(磁碟不足、檔案被佔用),其餘原樣。
pub fn map_io_error(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(112 | 39) => format!("磁碟空間不足,請清出空間後按「繼續」({e})"),
        Some(5 | 32 | 33) => format!("檔案被其他程式佔用(可能是防毒掃描),請稍後重試({e})"),
        _ if e.kind() == io::ErrorKind::StorageFull => format!("磁碟空間不足,請清出空間後按「繼續」({e})"),
        _ => e.to_string(),
    }
}

/// 一個下載任務的控制與進度快照(前端重載後可查回)。
#[derive(Debug, Default)]
pub struct DownloadCtl {
    pub cancel: AtomicBool,
    pub done: AtomicU64,
    pub total: AtomicU64,
}

/// 進行中的下載;同一個模型同時只能有一個任務。
#[derive(Debug, Default)]
pub struct DownloadRegistry {
    active: Mutex<HashMap<String, Arc<DownloadCtl>>>,
}

/// 下載任務的登記;drop(含 panic 展開)時自動移除。
pub struct DownloadGuard {
    registry: Arc<DownloadRegistry>,
    id: String,
    ctl: Arc<DownloadCtl>,
}

impl DownloadRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<DownloadCtl>>> {
        // 持鎖期間 panic 也不能讓登記表永久失效
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 在同一把鎖內檢查並登記;該模型已在下載中回 None。
    pub fn try_begin(self: &Arc<Self>, id: &str) -> Option<DownloadGuard> {
        let mut active = self.lock();
        if active.contains_key(id) {
            return None;
        }
        let ctl = Arc::new(DownloadCtl::default());
        active.insert(id.to_string(), ctl.clone());
        Some(DownloadGuard { registry: self.clone(), id: id.to_string(), ctl })
    }

    /// 要求取消;沒有在下載回 false。
    pub fn cancel(&self, id: &str) -> bool {
        match self.lock().get(id) {
            Some(c) => {
                c.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn cancel_all(&self) {
        for c in self.lock().values() {
            c.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// 進行中任務的 (已下載, 總量)。
    pub fn snapshot(&self, id: &str) -> Option<(u64, u64)> {
        self.lock()
            .get(id)
            .map(|c| (c.done.load(Ordering::Relaxed), c.total.load(Ordering::Relaxed)))
    }
}

impl DownloadGuard {
    pub fn ctl(&self) -> Arc<DownloadCtl> {
        self.ctl.clone()
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        self.registry.lock().remove(&self.id);
    }
}

fn is_transient_lock(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// 改名;遇到防毒掃描等暫時鎖檔(OS 錯誤 5/32/33)時以遞增間隔重試,最長 `max_wait`。
pub fn retry_rename(from: &Path, to: &Path, max_wait: Duration) -> io::Result<()> {
    let t0 = Instant::now();
    let mut wait = Duration::from_millis(50);
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_lock(&e) && t0.elapsed() + wait <= max_wait => {
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(1));
            }
            Err(e) => return Err(e),
        }
    }
}

/// 改名的重試上限(防毒掃描通常幾百毫秒內放開)。
const RENAME_MAX_WAIT: Duration = Duration::from_secs(10);

/// 下載參數(測試可縮短逾時與重試間隔)。
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub base_url: String,
    pub connect_timeout: Duration,
    /// 單次讀取逾時;不設總逾時(大檔在慢網路要下很久)。
    pub read_timeout: Duration,
    /// 暫時性錯誤(斷線、429、5xx)自動重試次數。
    pub retries: u32,
    pub retry_backoff: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            base_url: base_url(),
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(60),
            retries: 3,
            retry_backoff: Duration::from_secs(1),
        }
    }
}

/// 下載失敗的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    /// 大小或 SHA256 不符(暫存檔已刪,重試會從頭下載)。
    Verify { file: String },
    Http { status: u16, file: String },
    Network(String),
    Io(String),
    DiskFull { need: u64, free: u64 },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => write!(f, "已取消,下次按「繼續」會從中斷處接著下載"),
            DownloadError::Verify { file } => write!(f, "{file} 校驗失敗(內容與預期不符),已刪除暫存檔,請重試"),
            DownloadError::Http { status, file } => write!(f, "下載 {file} 失敗:伺服器回應 HTTP {status}"),
            DownloadError::Network(e) => write!(f, "無法連線 Hugging Face:{e}"),
            DownloadError::Io(e) => write!(f, "{e}"),
            DownloadError::DiskFull { need, free } => {
                write!(f, "磁碟空間不足:需要約 {} MB,剩 {} MB", need / 1_000_000, free / 1_000_000)
            }
        }
    }
}

fn io_err(e: io::Error) -> DownloadError {
    DownloadError::Io(map_io_error(&e))
}

fn staging_dir(spec: &ModelSpec, models_dir: &Path) -> PathBuf {
    models_dir.join(".download").join(spec.dir_name)
}

fn part_path(staging: &Path, f: &FileSpec) -> PathBuf {
    staging.join(format!("{}.part", f.name))
}

/// 已經有的位元組(已提交的檔案算全部,暫存檔算已寫入的部分)。
fn bytes_present(staging: &Path, f: &FileSpec) -> u64 {
    if file_len(&staging.join(f.name)) == Some(f.size) {
        return f.size;
    }
    file_len(&part_path(staging, f)).filter(|n| *n <= f.size).unwrap_or(0)
}

/// 尚未下載的位元組數(已提交的檔案與暫存檔已有的部分不算)。
pub fn bytes_needed(spec: &ModelSpec, models_dir: &Path) -> u64 {
    let staging = staging_dir(spec, models_dir);
    spec.files.iter().map(|f| f.size - bytes_present(&staging, f)).sum()
}

/// 磁碟剩餘空間(位元組);查不到回 None。
#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut avail = 0u64;
    // SAFETY:wide 以 0 結尾;三個輸出指標都指向有效的 u64(後兩個允許為 null,這裡給有效位址)。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(avail)
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// 開始前檢查磁碟空間(剩餘未下載量 + 64 MB 餘裕)。
pub fn check_disk_space(spec: &ModelSpec, models_dir: &Path) -> Result<(), DownloadError> {
    fs::create_dir_all(models_dir).map_err(io_err)?;
    let need = bytes_needed(spec, models_dir) + 64 * 1_000_000;
    match free_space(models_dir) {
        Some(free) if free < need => Err(DownloadError::DiskFull { need, free }),
        _ => Ok(()),
    }
}

fn sha256_file(p: &Path) -> io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 下載中的累計狀態與節流後的進度回報。
struct Reporter<'a> {
    id: &'a str,
    total: u64,
    done: u64,
    throttle: ProgressThrottle,
    speed_mark: (Instant, u64),
    bps: u64,
    ctl: &'a DownloadCtl,
    on_progress: &'a mut dyn FnMut(Progress),
}

impl Reporter<'_> {
    fn emit(&mut self, phase: Phase, message: Option<String>) {
        (self.on_progress)(Progress { id: self.id.to_string(), phase, done: self.done, total: self.total, bps: self.bps, message });
    }

    fn set_done(&mut self, done: u64) {
        self.done = done;
        self.ctl.done.store(done, Ordering::Relaxed);
        let now = Instant::now();
        if self.throttle.should_emit(now, done) {
            // 速度取至少 1 秒的平均,單次讀取的快慢不會讓數字亂跳
            let (t, d) = self.speed_mark;
            let secs = now.saturating_duration_since(t).as_secs_f64();
            if secs >= 1.0 {
                self.bps = (done.saturating_sub(d) as f64 / secs) as u64;
                self.speed_mark = (now, done);
            }
            self.emit(Phase::Downloading, None);
        }
    }
}

fn build_agent(opts: &DownloadOptions) -> Result<ureq::Agent, DownloadError> {
    let tls = native_tls::TlsConnector::new().map_err(|e| DownloadError::Network(format!("TLS 初始化失敗:{e}")))?;
    Ok(ureq::AgentBuilder::new()
        .timeout_connect(opts.connect_timeout)
        .timeout_read(opts.read_timeout)
        .user_agent(concat!("tingyi/", env!("CARGO_PKG_VERSION")))
        .tls_connector(Arc::new(tls))
        .build())
}

/// 把一個檔案下載到 `.part`(含續傳、自動重試);回傳時 `.part` 長度等於 `f.size`。
fn fetch_part(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    f: &FileSpec,
    opts: &DownloadOptions,
    rep: &mut Reporter,
) -> Result<(), DownloadError> {
    let base_done = rep.done - file_len(part).unwrap_or(0).min(f.size);
    let mut attempt = 0u32;
    let mut best = file_len(part).unwrap_or(0); // 暫存檔達過的最大長度:只有超過它才算真的有進度
    let ctl = rep.ctl;
    let retry = |attempt: &mut u32, err: DownloadError| -> Result<(), DownloadError> {
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        *attempt += 1;
        if *attempt > opts.retries {
            return Err(err);
        }
        // 退避等待切小段輪詢取消旗標,取消時不必等完
        let deadline = Instant::now() + opts.retry_backoff * *attempt;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if ctl.cancel.load(Ordering::SeqCst) {
                return Err(DownloadError::Cancelled);
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
        Ok(())
    };
    loop {
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let have = file_len(part).unwrap_or(0);
        if have == f.size {
            return Ok(());
        }
        let from = if f.size < SMALL_FILE { 0 } else { have };
        let mut req = agent.get(url);
        if from > 0 {
            req = req.set("Range", &format!("bytes={from}-"));
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                retry(&mut attempt, DownloadError::Network(t.to_string()))?;
                continue;
            }
        };
        let status = resp.status();
        let content_length = resp.header("Content-Length").and_then(|v| v.trim().parse::<u64>().ok());
        let mut file = match classify_response(status, resp.header("Content-Range"), content_length, from, f.size) {
            ResponseAction::Append => OpenOptions::new().create(true).append(true).open(part).map_err(io_err)?,
            ResponseAction::Truncate => File::create(part).map_err(io_err)?,
            ResponseAction::Restart => {
                let _ = fs::remove_file(part);
                retry(&mut attempt, DownloadError::Http { status, file: f.name.to_string() })?;
                continue;
            }
            ResponseAction::Retry => {
                retry(&mut attempt, DownloadError::Http { status, file: f.name.to_string() })?;
                continue;
            }
            ResponseAction::Fail(_) => return Err(DownloadError::Http { status, file: f.name.to_string() }),
        };
        let mut written = file.metadata().map_err(io_err)?.len();
        rep.set_done(base_done + written);
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let read_result: Result<(), io::Error> = loop {
            if rep.ctl.cancel.load(Ordering::SeqCst) {
                let _ = file.flush();
                return Err(DownloadError::Cancelled);
            }
            match reader.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if written + n as u64 > f.size {
                        break Err(io::Error::new(io::ErrorKind::InvalidData, "伺服器送出的內容比預期長"));
                    }
                    file.write_all(&buf[..n]).map_err(io_err)?;
                    written += n as u64;
                    rep.set_done(base_done + written);
                }
                Err(e) => break Err(e),
            }
        };
        file.sync_data().map_err(io_err)?;
        drop(file);
        if written > best {
            best = written;
            // 重試上限算的是「連續沒有進度」的次數,大檔在弱網路斷幾次仍能下完;
            // 伺服器多送內容(InvalidData)或從頭重下(淨進度為 0)都不算進度
            if !matches!(&read_result, Err(e) if e.kind() == io::ErrorKind::InvalidData) {
                attempt = 0;
            }
        }
        match read_result {
            Ok(()) if written == f.size => return Ok(()),
            Ok(()) => retry(&mut attempt, DownloadError::Network(format!("{} 連線提早結束", f.name)))?,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let _ = fs::remove_file(part);
                retry(&mut attempt, DownloadError::Network(e.to_string()))?;
            }
            Err(e) => retry(&mut attempt, DownloadError::Network(e.to_string()))?,
        }
    }
}

/// 把通過校驗的檔案放進正式目錄:目錄不存在就整個改名過去,否則逐檔移入(不刪使用者的檔案)。
fn install(spec: &ModelSpec, staging: &Path, models_dir: &Path) -> Result<(), DownloadError> {
    let final_dir = models_dir.join(spec.dir_name);
    if final_dir.exists() || retry_rename(staging, &final_dir, RENAME_MAX_WAIT).is_err() {
        fs::create_dir_all(&final_dir).map_err(io_err)?;
        for f in spec.files {
            let src = staging.join(f.name);
            if src.exists() {
                let dst = final_dir.join(f.name);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent).map_err(io_err)?;
                }
                // Windows 的 rename 會取代既有檔案
                retry_rename(&src, &dst, RENAME_MAX_WAIT).map_err(io_err)?;
            }
        }
        let _ = fs::remove_dir_all(staging);
    }
    // 壓縮檔留在目錄裡(就緒判斷要看它的大小),內容解到同一個目錄
    if spec.unzip {
        for f in spec.files.iter().filter(|f| f.name.ends_with(".zip")) {
            extract_zip(&final_dir.join(f.name), &final_dir).map_err(io_err)?;
        }
        fs::write(final_dir.join(UNZIP_DONE_MARKER), b"").map_err(io_err)?;
    }
    Ok(())
}

/// 翻譯套件實際能不能用(不論是 app 內下載或 `scripts/download-models.ps1` 放的位置):
/// llama.cpp 看 `<data>/bin/llama/llama-server.exe`,翻譯模型看 `paths::find_gguf`;其他模型照 `model_status`。
pub fn package_usable(spec: &ModelSpec, data_dir: &Path) -> bool {
    if spec.id == LLAMA_CPP.id {
        data_dir.join("bin").join("llama").join("llama-server.exe").is_file()
    } else if spec.id == HY_MT2_1_8B.id {
        let stem = HY_MT2_1_8B.files[0].name.trim_end_matches(".gguf");
        crate::paths::find_gguf(&data_dir.join("models"), stem).is_file()
    } else if spec.id == SENSEVOICE.id {
        // 與主視窗「開始」的條件一致:兩個檔都在就能辨識(下載腳本與 app 內下載放在同一個目錄)
        let dir = data_dir.join("models").join(SENSEVOICE.dir_name);
        SENSEVOICE.files.iter().all(|f| dir.join(f.name).is_file())
    } else if spec.id == WHISPER_SERVER.id {
        // 自己編的放 bin/whisper/;官方 zip 解在 bin/whisper/Release/,要解壓完成標記才算(解到一半不算)
        let dir = data_dir.join("bin").join("whisper");
        dir.join("whisper-server.exe").is_file()
            || (dir.join("Release").join("whisper-server.exe").is_file() && dir.join(UNZIP_DONE_MARKER).is_file())
    } else if spec.id == SILERO_VAD.id || spec.id == SPEAKER.id || spec.id == WHISPER_TURBO.id {
        // 下載腳本放在 models 根目錄,app 內下載放在子資料夾:兩處都算
        crate::paths::find_model_file(&data_dir.join("models"), spec.dir_name, spec.files[0].name).is_file()
    } else {
        model_status(spec, &data_dir.join("models")) == ModelStatus::Ready
    }
}

/// 套件要裝在哪個上層目錄:llama.cpp 在 `bin_dir()`,模型在 `models_dir()`。
pub fn install_parent(spec: &ModelSpec) -> PathBuf {
    if spec.id == LLAMA_CPP.id || spec.id == WHISPER_SERVER.id {
        crate::paths::bin_dir()
    } else {
        crate::paths::models_dir()
    }
}

/// 下載整個模型到 `models_dir/<dir_name>`;同步執行。
pub fn download_model(
    spec: &ModelSpec,
    models_dir: &Path,
    opts: &DownloadOptions,
    ctl: &DownloadCtl,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    let staging = staging_dir(spec, models_dir);
    fs::create_dir_all(&staging).map_err(io_err)?;
    let total = spec.total_size();
    ctl.total.store(total, Ordering::Relaxed);
    let present: u64 = spec.files.iter().map(|f| bytes_present(&staging, f)).sum();
    let mut rep = Reporter {
        id: spec.id,
        total,
        done: present,
        throttle: ProgressThrottle::new(total),
        speed_mark: (Instant::now(), present),
        bps: 0,
        ctl,
        on_progress,
    };
    rep.set_done(present);
    let mut agent: Option<ureq::Agent> = None;
    for f in spec.files {
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let committed = staging.join(f.name);
        let part = part_path(&staging, f);
        // 檔名可含子資料夾(例如 `tokenizer/vocab.json`);只在下載時建,查狀態不動磁碟
        if let Some(parent) = part.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let committed_len = file_len(&committed);
        if committed_len.is_some_and(|n| n != f.size) {
            let _ = fs::remove_file(&committed);
        }
        match plan_file(file_len(&part), committed_len, f.size) {
            FilePlan::Skip => continue,
            FilePlan::Restart => {
                // 比應有的大:bytes_present 本來就沒把它算進進度
                let _ = fs::remove_file(&part);
            }
            FilePlan::Fresh if file_len(&part).is_some_and(|n| n > 0) => {
                // 小檔的部分暫存檔不續傳:扣回進度後從頭下載
                rep.done -= file_len(&part).unwrap_or(0).min(f.size);
                let _ = fs::remove_file(&part);
            }
            FilePlan::Fresh | FilePlan::Resume(_) | FilePlan::VerifyOnly => {}
        }
        if file_len(&part) != Some(f.size) {
            let a = match &agent {
                Some(a) => a,
                None => agent.insert(build_agent(opts)?),
            };
            fetch_part(a, &spec.url(&opts.base_url, f), &part, f, opts, &mut rep)?;
        }
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        rep.emit(Phase::Verifying, Some(f.name.to_string()));
        let ok = sha256_file(&part).map_err(io_err)? == f.sha256 && file_len(&part) == Some(f.size);
        if !ok {
            let _ = fs::remove_file(&part);
            return Err(DownloadError::Verify { file: f.name.to_string() });
        }
        retry_rename(&part, &committed, RENAME_MAX_WAIT).map_err(io_err)?;
        rep.speed_mark = (Instant::now(), rep.done); // 校驗時間不算進下載速度
    }
    install(spec, &staging, models_dir)?;
    if let Some(parent) = staging.parent() {
        let _ = fs::remove_dir(parent); // `.download` 空了才刪得掉
    }
    rep.done = total;
    ctl.done.store(total, Ordering::Relaxed);
    rep.emit(Phase::Done, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    const BIG_PATH: &str = "/r/resolve/c/big.bin";
    const SMALL_PATH: &str = "/r/resolve/c/small.txt";

    // ---------- 測試用規格與資料 ----------

    struct Fx {
        spec: &'static ModelSpec,
        big: Vec<u8>,
        small: Vec<u8>,
    }

    fn hex(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    /// 2 MiB 偽隨機大檔 + 10 bytes 小檔;規格用 Box::leak 取得 'static。
    fn fx() -> Fx {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let big: Vec<u8> = (0..(2 * SMALL_FILE as usize))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let small = b"0123456789".to_vec();
        let files: &'static [FileSpec] = Box::leak(
            vec![
                FileSpec {
                    name: "big.bin",
                    size: big.len() as u64,
                    sha256: Box::leak(hex(&big).into_boxed_str()),
                },
                FileSpec {
                    name: "small.txt",
                    size: small.len() as u64,
                    sha256: Box::leak(hex(&small).into_boxed_str()),
                },
            ]
            .into_boxed_slice(),
        );
        let spec: &'static ModelSpec = Box::leak(Box::new(ModelSpec {
            id: "t",
            lang: Lang::Ja,
            label: "t",
            dir_name: "tmodel",
            repo: "r",
            commit: "c",
            license: "l",
            license_url: "u",
            files,
            url_base: None,
            unzip: false,
            ready_marker: None,
        }));
        Fx { spec, big, small }
    }

    fn files_map(fx: &Fx) -> HashMap<String, Vec<u8>> {
        let mut m = HashMap::new();
        m.insert(BIG_PATH.to_string(), fx.big.clone());
        m.insert(SMALL_PATH.to_string(), fx.small.clone());
        m
    }

    fn opts(base: &str) -> DownloadOptions {
        DownloadOptions {
            base_url: base.to_string(),
            connect_timeout: Duration::from_secs(2),
            read_timeout: Duration::from_millis(500),
            retries: 3,
            retry_backoff: Duration::from_millis(10),
        }
    }

    fn make_file(dir: &Path, name: &str, size: u64) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::File::create(dir.join(name)).unwrap().set_len(size).unwrap();
    }

    fn part_path(models: &Path, name: &str) -> PathBuf {
        models.join(".download").join("tmodel").join(format!("{name}.part"))
    }

    fn put_part(models: &Path, name: &str, data: &[u8]) {
        let p = part_path(models, name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(files_under(&p));
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    fn run(fx: &Fx, models: &Path, o: &DownloadOptions) -> (Result<(), DownloadError>, Vec<Progress>) {
        std::fs::create_dir_all(models).unwrap();
        let ctl = DownloadCtl::default();
        let mut ps: Vec<Progress> = Vec::new();
        let r = download_model(fx.spec, models, o, &ctl, &mut |p: Progress| ps.push(p));
        (r, ps)
    }

    // ---------- 假 HTTP 伺服器(只 mock 網路) ----------

    #[derive(Debug, Clone)]
    struct Req {
        path: String,
        headers: Vec<(String, String)>,
    }

    impl Req {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
        }
    }

    fn read_request(stream: &mut TcpStream) -> Option<Req> {
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut tmp).ok()?;
            if n == 0 || buf.len() > 64 * 1024 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let mut lines = text.split("\r\n");
        let path = lines.next()?.split_whitespace().nth(1)?.to_string();
        let headers = lines
            .take_while(|l| !l.is_empty())
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
            })
            .collect();
        Some(Req { path, headers })
    }

    fn send_head(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], content_len: usize) {
        let reason = match status {
            200 => "OK",
            206 => "Partial Content",
            307 => "Temporary Redirect",
            404 => "Not Found",
            503 => "Service Unavailable",
            _ => "Status",
        };
        let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str(&format!("Content-Length: {content_len}\r\nConnection: close\r\n\r\n"));
        let _ = stream.write_all(head.as_bytes());
    }

    /// `content_len` 可大於 `body.len()`:模擬送到一半就斷線。
    fn send(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], content_len: usize, body: &[u8]) {
        send_head(stream, status, headers, content_len);
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    /// 遵守(或忽略)Range 的一般檔案回應;路徑不在表中回 404。
    fn serve_file(files: &HashMap<String, Vec<u8>>, honor_range: bool, req: &Req, stream: &mut TcpStream) {
        let Some(body) = files.get(&req.path) else {
            send(stream, 404, &[], 0, b"");
            return;
        };
        let total = body.len();
        if honor_range {
            let start = req
                .header("range")
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|v| v.strip_suffix('-'))
                .and_then(|v| v.parse::<usize>().ok());
            if let Some(start) = start.filter(|s| *s < total) {
                let cr = format!("bytes {}-{}/{}", start, total - 1, total);
                send(stream, 206, &[("Content-Range", cr)], total - start, &body[start..]);
                return;
            }
        }
        send(stream, 200, &[], total, body);
    }

    struct Server {
        base: String,
        reqs: Arc<Mutex<Vec<Req>>>,
        stop: Arc<AtomicBool>,
    }

    impl Server {
        /// `make` 拿到伺服器的 base url,回傳處理函式 `(請求, 同路徑第幾次請求(從 0), 連線)`。
        fn start<H>(make: impl FnOnce(&str) -> H) -> Server
        where
            H: Fn(&Req, usize, &mut TcpStream) + Send + Sync + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let handler = Arc::new(make(&base));
            let reqs: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            {
                let (reqs, stop) = (reqs.clone(), stop.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut s, _)) => {
                                let (handler, reqs) = (handler.clone(), reqs.clone());
                                std::thread::spawn(move || {
                                    let _ = s.set_nonblocking(false);
                                    let Some(req) = read_request(&mut s) else { return };
                                    let n = {
                                        let mut g = reqs.lock().unwrap();
                                        let n = g.iter().filter(|r| r.path == req.path).count();
                                        g.push(req.clone());
                                        n
                                    };
                                    handler(&req, n, &mut s);
                                    let _ = s.shutdown(Shutdown::Write);
                                });
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(2)),
                        }
                    }
                });
            }
            Server { base, reqs, stop }
        }

        fn requests(&self) -> Vec<Req> {
            self.reqs.lock().unwrap().clone()
        }

        fn requests_for(&self, path: &str) -> Vec<Req> {
            self.requests().into_iter().filter(|r| r.path == path).collect()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn files_server(files: HashMap<String, Vec<u8>>, honor_range: bool) -> Server {
        Server::start(move |_| move |req: &Req, _n: usize, s: &mut TcpStream| serve_file(&files, honor_range, req, s))
    }

    // ---------- T1:model_status / dir_ready ----------

    // T1:缺一個檔案時為 Partial 且列出缺的檔名
    #[test]
    fn status_missing_file_is_partial() {
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        make_file(&tmp.path().join("tmodel"), "big.bin", fx.big.len() as u64);
        match model_status(fx.spec, tmp.path()) {
            ModelStatus::Partial { missing } => assert!(missing.iter().any(|m| m == "small.txt"), "實得 {missing:?}"),
            other => panic!("應為 Partial,實得 {other:?}"),
        }
    }

    // T1:某檔大小差 1 時為 Partial 且列出該檔
    #[test]
    fn status_wrong_size_is_partial() {
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tmodel");
        make_file(&dir, "big.bin", fx.big.len() as u64 + 1);
        make_file(&dir, "small.txt", fx.small.len() as u64);
        match model_status(fx.spec, tmp.path()) {
            ModelStatus::Partial { missing } => assert!(missing.iter().any(|m| m == "big.bin"), "實得 {missing:?}"),
            other => panic!("應為 Partial,實得 {other:?}"),
        }
        make_file(&dir, "big.bin", fx.big.len() as u64);
        make_file(&dir, "small.txt", fx.small.len() as u64 - 1);
        match model_status(fx.spec, tmp.path()) {
            ModelStatus::Partial { missing } => assert!(missing.iter().any(|m| m == "small.txt"), "實得 {missing:?}"),
            other => panic!("應為 Partial,實得 {other:?}"),
        }
    }

    // T1:檔案齊全且大小正確(多一個無關檔)為 Ready
    #[test]
    fn status_complete_is_ready_even_with_extra_file() {
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tmodel");
        make_file(&dir, "big.bin", fx.big.len() as u64);
        make_file(&dir, "small.txt", fx.small.len() as u64);
        make_file(&dir, "user.txt", 3);
        assert_eq!(model_status(fx.spec, tmp.path()), ModelStatus::Ready);
    }

    // T1:目錄不存在為 Absent
    #[test]
    fn status_missing_dir_is_absent() {
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(model_status(fx.spec, tmp.path()), ModelStatus::Absent);
    }

    // T1:dir_ready 對不在規格表的目錄只看存在與否,在規格表的目錄要檔案齊全
    #[test]
    fn dir_ready_unknown_dir_checks_existence_only() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!dir_ready(tmp.path(), "bench-ja-en"));
        std::fs::create_dir_all(tmp.path().join("bench-ja-en")).unwrap();
        assert!(dir_ready(tmp.path(), "bench-ja-en"));
        // 規格表內的目錄:空目錄不算就緒
        let known = PARAKEET.dir_name;
        std::fs::create_dir_all(tmp.path().join(known)).unwrap();
        assert!(!dir_ready(tmp.path(), known));
    }

    // T1 補充:總大小是各檔大小之和、網址格式為 base/repo/resolve/commit/name
    #[test]
    fn spec_total_size_and_url() {
        let fx = fx();
        assert_eq!(fx.spec.total_size(), (fx.big.len() + fx.small.len()) as u64);
        assert_eq!(fx.spec.url("http://h:1", &fx.spec.files[0]), "http://h:1/r/resolve/c/big.bin");
    }

    // ---------- T2:plan_file ----------

    // T2:續傳計畫依暫存檔長度與已提交長度決定
    #[test]
    fn plan_file_by_part_length() {
        let s = 10 * 1024 * 1024u64;
        let mb3 = 3 * 1024 * 1024u64;
        assert_eq!(plan_file(None, None, s), FilePlan::Fresh);
        assert_eq!(plan_file(Some(0), None, s), FilePlan::Fresh);
        assert_eq!(plan_file(Some(mb3), None, s), FilePlan::Resume(mb3));
        assert_eq!(plan_file(Some(s), None, s), FilePlan::VerifyOnly);
        assert_eq!(plan_file(Some(s + 1), None, s), FilePlan::Restart);
        assert_eq!(plan_file(None, Some(s), s), FilePlan::Skip);
    }

    // T2:小檔有部分暫存檔也從頭下載
    #[test]
    fn plan_file_small_file_always_fresh() {
        let s = SMALL_FILE - 1;
        assert_eq!(plan_file(Some(100), None, s), FilePlan::Fresh);
    }

    // ---------- T3:parse_content_range ----------

    // T3:正確解析 Content-Range,無法解析時回 None
    #[test]
    fn parse_content_range_cases() {
        assert_eq!(parse_content_range("bytes 10-99/100"), Some((10, 99, 100)));
        assert_eq!(parse_content_range("bytes */100"), None);
        assert_eq!(parse_content_range(""), None);
        assert_eq!(parse_content_range("garbage"), None);
    }

    // ---------- T4:classify_response ----------

    // T4:206 且起點與總長正確才接續,否則重來
    #[test]
    fn classify_206_append_or_restart() {
        assert_eq!(classify_response(206, Some("bytes 1000-4999/5000"), Some(4000), 1000, 5000), ResponseAction::Append);
        assert_eq!(classify_response(206, Some("bytes 0-4999/5000"), Some(5000), 1000, 5000), ResponseAction::Restart);
        assert_eq!(classify_response(206, Some("bytes 1000-4999/6000"), Some(4000), 1000, 5000), ResponseAction::Restart);
    }

    // T4:200(忽略 Range)截斷重寫,416 重來
    #[test]
    fn classify_200_truncate_and_416_restart() {
        assert_eq!(classify_response(200, None, Some(5000), 1000, 5000), ResponseAction::Truncate);
        assert_eq!(classify_response(200, None, None, 1000, 5000), ResponseAction::Truncate);
        assert_eq!(classify_response(416, None, None, 1000, 5000), ResponseAction::Restart);
    }

    // T4:404 不可重試,429 與 5xx 可重試
    #[test]
    fn classify_errors_fail_or_retry() {
        match classify_response(404, None, None, 0, 5000) {
            ResponseAction::Fail(msg) => assert!(msg.contains("404"), "實得 {msg:?}"),
            other => panic!("404 應為 Fail,實得 {other:?}"),
        }
        for st in [503u16, 429, 500] {
            assert_eq!(classify_response(st, None, None, 0, 5000), ResponseAction::Retry, "狀態 {st}");
        }
    }

    // ---------- T5:ProgressThrottle ----------

    // T5:進度節流:短時間小變化不發,變化 ≥1% 或過 250 ms 才發,完成一律發
    #[test]
    fn throttle_emits_on_first_percent_interval_and_done() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut t = ProgressThrottle::new(1000);
        assert!(t.should_emit(t0, 0), "第一次要發");
        assert!(!t.should_emit(t0 + ms(10), 1), "10 ms 內小變化不發");
        assert!(t.should_emit(t0 + ms(20), 12), "變化 ≥1% 要發");
        assert!(!t.should_emit(t0 + ms(30), 13), "剛發過、小變化不發");
        assert!(t.should_emit(t0 + ms(20 + 300), 14), "過 300 ms 小變化要發");
        assert!(t.should_emit(t0 + ms(321), 1000), "完成一律發");
    }

    // ---------- T6:map_io_error ----------

    // T6:磁碟不足的 OS 錯誤碼顯示友善訊息
    #[test]
    fn map_io_error_disk_full() {
        for code in [112, 39] {
            let m = map_io_error(&io::Error::from_raw_os_error(code));
            assert!(m.contains("磁碟空間不足"), "碼 {code} 實得 {m:?}");
            assert!(!m.contains("佔用"), "碼 {code} 實得 {m:?}");
        }
    }

    // T6:檔案被佔用的 OS 錯誤碼顯示友善訊息
    #[test]
    fn map_io_error_locked() {
        for code in [5, 32, 33] {
            let m = map_io_error(&io::Error::from_raw_os_error(code));
            assert!(m.contains("佔用"), "碼 {code} 實得 {m:?}");
            assert!(!m.contains("磁碟空間不足"), "碼 {code} 實得 {m:?}");
        }
    }

    // T6:其他錯誤原樣輸出,不含友善訊息
    #[test]
    fn map_io_error_other_passthrough() {
        let e = io::Error::new(io::ErrorKind::Other, "boom");
        let m = map_io_error(&e);
        assert!(m.contains("boom"));
        assert!(!m.contains("磁碟空間不足") && !m.contains("佔用"), "實得 {m:?}");
        let m2 = map_io_error(&io::Error::from_raw_os_error(2));
        assert!(!m2.contains("磁碟空間不足") && !m2.contains("佔用"), "實得 {m2:?}");
    }

    // ---------- T7:DownloadRegistry ----------

    // T7:同模型不可重複登記,不同模型可,guard drop 後可再登記
    #[test]
    fn registry_rejects_duplicate_until_dropped() {
        let reg = Arc::new(DownloadRegistry::default());
        let g = reg.try_begin("a");
        assert!(g.is_some(), "第一次登記應成功");
        assert!(reg.try_begin("a").is_none(), "同 id 第二次應被拒");
        assert!(reg.try_begin("b").is_some(), "不同 id 可登記");
        drop(g);
        assert!(reg.try_begin("a").is_some(), "guard drop 後可再登記");
    }

    // T7:持有 guard 時 panic 展開後仍會移除登記
    #[test]
    fn registry_released_after_panic() {
        let reg = Arc::new(DownloadRegistry::default());
        let r2 = reg.clone();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _g = r2.try_begin("a").expect("應登記成功");
            panic!("模擬下載中 panic");
        }));
        assert!(res.is_err());
        assert!(reg.try_begin("a").is_some(), "panic 後應可再登記");
    }

    // T7:cancel 對進行中的任務回 true 並設定旗標,沒在下載回 false
    #[test]
    fn registry_cancel_sets_flag() {
        let reg = Arc::new(DownloadRegistry::default());
        let g = reg.try_begin("a").expect("應登記成功");
        assert!(!g.ctl().cancel.load(Ordering::SeqCst));
        assert!(reg.cancel("a"));
        assert!(g.ctl().cancel.load(Ordering::SeqCst));
        assert!(!reg.cancel("zzz"));
    }

    // T7:snapshot 回進行中任務的 done/total,結束後為 None
    #[test]
    fn registry_snapshot_reports_progress() {
        let reg = Arc::new(DownloadRegistry::default());
        assert_eq!(reg.snapshot("a"), None);
        let g = reg.try_begin("a").expect("應登記成功");
        g.ctl().done.store(5, Ordering::SeqCst);
        g.ctl().total.store(10, Ordering::SeqCst);
        assert_eq!(reg.snapshot("a"), Some((5, 10)));
        drop(g);
        assert_eq!(reg.snapshot("a"), None);
    }

    // ---------- T8:retry_rename(僅 Windows) ----------

    // T8:來源檔被暫時鎖住 300 ms 後釋放,改名重試後成功
    #[cfg(windows)]
    #[test]
    fn retry_rename_succeeds_after_lock_released() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.part");
        let to = dir.path().join("a.bin");
        std::fs::write(&from, b"x").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let f2 = from.clone();
        let h = std::thread::spawn(move || {
            let f = std::fs::OpenOptions::new().read(true).share_mode(0).open(&f2).unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            drop(f);
        });
        rx.recv().unwrap();
        retry_rename(&from, &to, Duration::from_secs(3)).expect("釋放鎖後應改名成功");
        assert!(to.exists());
        assert!(!from.exists());
        h.join().unwrap();
    }

    // T8:鎖住不放時超過 max_wait 回錯誤
    #[cfg(windows)]
    #[test]
    fn retry_rename_fails_when_lock_held() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.part");
        let to = dir.path().join("a.bin");
        std::fs::write(&from, b"x").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (rel_tx, rel_rx) = std::sync::mpsc::channel::<()>();
        let f2 = from.clone();
        let h = std::thread::spawn(move || {
            let f = std::fs::OpenOptions::new().read(true).share_mode(0).open(&f2).unwrap();
            tx.send(()).unwrap();
            let _ = rel_rx.recv();
            drop(f);
        });
        rx.recv().unwrap();
        let start = Instant::now();
        let r = retry_rename(&from, &to, Duration::from_millis(500));
        assert!(r.is_err(), "鎖住不放應回錯誤");
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(!to.exists());
        rel_tx.send(()).unwrap();
        h.join().unwrap();
    }

    // ---------- T9–T19:download_model(假 HTTP 伺服器) ----------

    // T9:全新下載落到正式目錄,暫存區清空,進度以 Done 結束
    #[test]
    fn download_fresh_commits_to_final_dir() {
        let fx = fx();
        let srv = files_server(files_map(&fx), true);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let (r, ps) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
        assert_eq!(std::fs::read(models.join("tmodel").join("big.bin")).unwrap(), fx.big);
        assert_eq!(std::fs::read(models.join("tmodel").join("small.txt")).unwrap(), fx.small);
        let left = files_under(&models.join(".download"));
        assert!(left.is_empty(), ".download 殘留 {left:?}");
        let last = ps.last().expect("至少要有一次進度");
        assert_eq!(last.phase, Phase::Done);
        let total = (fx.big.len() + fx.small.len()) as u64;
        assert_eq!(last.done, total);
        assert_eq!(last.total, total);
    }

    // T10:有暫存檔時以 Range: bytes=K- 從暫存檔長度續傳
    #[test]
    fn download_resumes_with_range_from_part_length() {
        let fx = fx();
        let k = 700_000usize;
        let srv = files_server(files_map(&fx), true);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        put_part(&models, "big.bin", &fx.big[..k]);
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        let big_reqs = srv.requests_for(BIG_PATH);
        assert_eq!(big_reqs.len(), 1, "大檔應只請求一次,實得 {big_reqs:?}");
        assert_eq!(big_reqs[0].header("range"), Some(format!("bytes={k}-").as_str()));
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
        assert_eq!(std::fs::read(models.join("tmodel").join("big.bin")).unwrap(), fx.big);
    }

    // T11:伺服器忽略 Range 回 200 全檔時從頭重寫,內容仍正確
    #[test]
    fn download_restarts_when_server_ignores_range() {
        let fx = fx();
        let srv = files_server(files_map(&fx), false);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        put_part(&models, "big.bin", &fx.big[..700_000]);
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
        assert_eq!(hex(&std::fs::read(models.join("tmodel").join("big.bin")).unwrap()), hex(&fx.big));
    }

    // T12:原網址 307 轉址後,新路徑的請求仍帶 Range
    #[test]
    fn download_keeps_range_after_redirect() {
        let fx = fx();
        let mut files = files_map(&fx);
        files.insert("/cdn/big.bin".to_string(), fx.big.clone());
        let k = 700_000usize;
        let srv = Server::start(move |base| {
            let loc = format!("{base}/cdn/big.bin");
            move |req: &Req, _n: usize, s: &mut TcpStream| {
                if req.path == BIG_PATH {
                    send(s, 307, &[("Location", loc.clone())], 0, b"");
                } else {
                    serve_file(&files, true, req, s);
                }
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        put_part(&models, "big.bin", &fx.big[..k]);
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        let cdn = srv.requests_for("/cdn/big.bin");
        assert!(!cdn.is_empty(), "應跟著轉址到 /cdn/big.bin");
        assert_eq!(cdn[0].header("range"), Some(format!("bytes={k}-").as_str()));
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
        assert_eq!(std::fs::read(models.join("tmodel").join("big.bin")).unwrap(), fx.big);
    }

    // T13:暫存檔已完整且正確時不發任何請求
    #[test]
    fn download_complete_parts_make_no_requests() {
        let fx = fx();
        let srv = files_server(files_map(&fx), true);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        put_part(&models, "big.bin", &fx.big);
        put_part(&models, "small.txt", &fx.small);
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        assert_eq!(srv.requests().len(), 0, "實得 {:?}", srv.requests());
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
    }

    // T14:伺服器送同長度的錯誤內容時回 Verify、刪暫存檔、不建立正式目錄
    #[test]
    fn download_tampered_content_fails_verify() {
        let fx = fx();
        let mut files = files_map(&fx);
        let mut bad = fx.big.clone();
        bad[1000] ^= 0xFF;
        files.insert(BIG_PATH.to_string(), bad);
        let srv = files_server(files, true);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        match r {
            Err(DownloadError::Verify { file }) => assert_eq!(file, "big.bin"),
            other => panic!("應為 Verify,實得 {other:?}"),
        }
        assert!(!part_path(&models, "big.bin").exists(), "校驗失敗應刪暫存檔");
        assert!(!models.join("tmodel").exists(), "正式目錄不應出現");
    }

    // T15:中途斷線後自動以 Range 從已收位元組續傳
    #[test]
    fn download_resumes_after_connection_drop() {
        let fx = fx();
        let files = files_map(&fx);
        let half = fx.big.len() / 2;
        let big = fx.big.clone();
        let srv = Server::start(move |_| {
            move |req: &Req, n: usize, s: &mut TcpStream| {
                if req.path == BIG_PATH && n == 0 {
                    send(s, 200, &[], big.len(), &big[..half]);
                } else {
                    serve_file(&files, true, req, s);
                }
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        let big_reqs = srv.requests_for(BIG_PATH);
        assert_eq!(big_reqs.len(), 2, "實得 {big_reqs:?}");
        assert_eq!(big_reqs[1].header("range"), Some(format!("bytes={half}-").as_str()));
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
        assert_eq!(std::fs::read(models.join("tmodel").join("big.bin")).unwrap(), fx.big);
    }

    // T16:取消時回 Cancelled、保留暫存檔、不建立正式目錄
    #[test]
    fn download_cancel_keeps_part_file() {
        let fx = fx();
        let files = files_map(&fx);
        let big = fx.big.clone();
        let srv = Server::start(move |_| {
            move |req: &Req, _n: usize, s: &mut TcpStream| {
                if req.path == BIG_PATH {
                    send_head(s, 200, &[], big.len());
                    for chunk in big.chunks(64 * 1024) {
                        if s.write_all(chunk).is_err() {
                            return;
                        }
                        let _ = s.flush();
                        std::thread::sleep(Duration::from_millis(50));
                    }
                } else {
                    serve_file(&files, true, req, s);
                }
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let ctl = DownloadCtl::default();
        let mut cb = |p: Progress| {
            if p.done > 0 {
                ctl.cancel.store(true, Ordering::SeqCst);
            }
        };
        let r = download_model(fx.spec, &models, &opts(&srv.base), &ctl, &mut cb);
        assert_eq!(r, Err(DownloadError::Cancelled));
        let part = part_path(&models, "big.bin");
        assert!(part.exists(), "取消後應保留暫存檔");
        assert!(std::fs::metadata(&part).unwrap().len() > 0);
        assert!(!models.join("tmodel").exists(), "正式目錄不應出現");
    }

    // T17:404 直接回報 Http 錯誤且不重試
    #[test]
    fn download_404_reports_http_error() {
        let fx = fx();
        let srv = files_server(HashMap::new(), true);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        match r {
            Err(DownloadError::Http { status, .. }) => assert_eq!(status, 404),
            other => panic!("應為 Http 404,實得 {other:?}"),
        }
        assert_eq!(srv.requests().len(), 1, "404 不應重試,實得 {:?}", srv.requests());
    }

    // T17:第一次 503、第二次 200 時自動重試成功
    #[test]
    fn download_retries_after_503() {
        let fx = fx();
        let files = files_map(&fx);
        let srv = Server::start(move |_| {
            move |req: &Req, n: usize, s: &mut TcpStream| {
                if n == 0 {
                    send(s, 503, &[], 0, b"");
                } else {
                    serve_file(&files, true, req, s);
                }
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        assert!(srv.requests_for(BIG_PATH).len() >= 2, "大檔應至少請求兩次");
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
    }

    // T18:正式目錄已存在時逐檔合併,不動使用者自己的檔案
    #[test]
    fn download_merges_into_existing_dir_keeping_user_files() {
        let fx = fx();
        let srv = files_server(files_map(&fx), true);
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let dir = models.join("tmodel");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("big.bin"), b"wrong-size").unwrap();
        std::fs::write(dir.join("user.txt"), "mine").unwrap();
        let (r, _) = run(&fx, &models, &opts(&srv.base));
        assert_eq!(r, Ok(()));
        assert_eq!(model_status(fx.spec, &models), ModelStatus::Ready);
        assert_eq!(std::fs::read_to_string(dir.join("user.txt")).unwrap(), "mine");
        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), fx.big);
    }

    // T19:伺服器連上後不回應時,在 read_timeout 後 5 秒內回錯誤而非卡死
    #[test]
    fn download_times_out_when_server_hangs() {
        let fx = fx();
        let srv = Server::start(|_| |_req: &Req, _n: usize, _s: &mut TcpStream| std::thread::sleep(Duration::from_millis(1500)));
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let mut o = opts(&srv.base);
        o.read_timeout = Duration::from_millis(300);
        o.retries = 0;
        let (tx, rx) = std::sync::mpsc::channel();
        let spec = fx.spec;
        let m2 = models.clone();
        std::thread::spawn(move || {
            let ctl = DownloadCtl::default();
            let r = download_model(spec, &m2, &o, &ctl, &mut |_p: Progress| {});
            let _ = tx.send(r);
        });
        let r = rx.recv_timeout(Duration::from_secs(5)).expect("5 秒內應因逾時回傳");
        assert!(
            matches!(r, Err(DownloadError::Network(_)) | Err(DownloadError::Io(_))),
            "應為 Network 或 Io 錯誤,實得 {r:?}"
        );
    }

    // T21:伺服器每次忽略 Range 且送一半就斷線時,重試次數有上限(不因每次有進度而歸零)
    #[test]
    fn download_retry_is_bounded_when_server_cuts_half_each_time() {
        let fx = fx();
        let big = fx.big.clone();
        let srv = Server::start(move |_| {
            move |req: &Req, n: usize, s: &mut TcpStream| {
                if req.path == BIG_PATH && n < 9 {
                    // 忽略 Range、宣稱完整大小、只送前一半
                    send(s, 200, &[], big.len(), &big[..big.len() / 2]);
                } else {
                    send(s, 404, &[], 0, b"");
                }
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let o = opts(&srv.base);
        let max_reqs = o.retries as usize + 1;
        let (r, _) = run(&fx, &models, &o);
        assert!(matches!(r, Err(DownloadError::Network(_))), "應為 Network 錯誤,實得 {r:?}");
        let n = srv.requests_for(BIG_PATH).len();
        assert!(n <= max_reqs, "大檔請求數應 <= {max_reqs},實得 {n}");
    }

    // T22:伺服器卡住時取消盡快回 Cancelled,且不再重送請求
    #[test]
    fn download_cancel_during_hang_returns_cancelled_without_retry() {
        let fx = fx();
        let srv = Server::start(|_| |_req: &Req, _n: usize, _s: &mut TcpStream| std::thread::sleep(Duration::from_millis(2000)));
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let mut o = opts(&srv.base);
        o.read_timeout = Duration::from_millis(300);
        o.retries = 3;
        o.retry_backoff = Duration::from_millis(10);
        let ctl = DownloadCtl::default();
        let start = std::time::Instant::now();
        let r = std::thread::scope(|sc| {
            sc.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                ctl.cancel.store(true, Ordering::SeqCst);
            });
            download_model(fx.spec, &models, &o, &ctl, &mut |_p: Progress| {})
        });
        let elapsed = start.elapsed();
        assert_eq!(r, Err(DownloadError::Cancelled), "實得 {r:?}");
        assert_eq!(srv.requests_for(BIG_PATH).len(), 1, "取消後不應重送請求");
        assert!(elapsed < Duration::from_secs(1), "應 1 秒內回傳,實得 {elapsed:?}");
    }

    // ---------- 翻譯套件(llama.cpp zip、Hy-MT2 gguf) ----------

    /// 從 scripts/download-models.ps1 取出 (Name, Url, Sha256);`$llamaBuild` 先代入。
    fn ps1_items() -> Vec<(String, String, String)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/download-models.ps1");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("讀不到 {}: {e}", path.display()));
        let build = regex::Regex::new(r"\$llamaBuild\s*=\s*'([^']+)'")
            .unwrap()
            .captures(&text)
            .map(|c| c[1].to_string())
            .expect("ps1 找不到 $llamaBuild");
        let re = regex::Regex::new(r#"(?s)Name = ["']([^"']+)["'];.*?Url = ["']([^"']+)["'];.*?Sha256 = '([0-9a-f]{64})'"#).unwrap();
        re.captures_iter(&text)
            .map(|c| {
                (
                    c[1].replace("$llamaBuild", &build),
                    c[2].replace("$llamaBuild", &build),
                    c[3].to_string(),
                )
            })
            .collect()
    }

    // T15:LLAMA_CPP、HY_MT2_1_8B 的檔名、網址、SHA256 與下載腳本一致
    #[test]
    fn t15_translate_packages_match_download_script() {
        let items = ps1_items();
        assert!(!items.is_empty(), "ps1 沒解析出任何項目");
        for spec in [&LLAMA_CPP, &HY_MT2_1_8B] {
            let f = &spec.files[0];
            let (_, url, sha) = items
                .iter()
                .find(|(n, _, _)| n == f.name)
                .unwrap_or_else(|| panic!("ps1 找不到 {}", f.name));
            assert_eq!(&spec.url(DEFAULT_BASE_URL, f), url, "{} 網址與 ps1 不同", spec.id);
            assert_eq!(f.sha256, sha, "{} SHA256 與 ps1 不同", spec.id);
        }
        assert!(TRANSLATE_PACKAGES.iter().any(|s| s.id == LLAMA_CPP.id));
        assert!(TRANSLATE_PACKAGES.iter().any(|s| s.id == HY_MT2_1_8B.id));
    }

    // T15b:有 url_base 時網址不理會 base;沒有時維持 HF 格式
    #[test]
    fn t15b_url_uses_url_base_when_present() {
        let f = &LLAMA_CPP.files[0];
        let u = LLAMA_CPP.url("http://ignored.example", f);
        assert!(!u.contains("ignored.example"), "url_base 應蓋過 base,實際 {u}");
        assert_eq!(u, format!("https://github.com/ggml-org/llama.cpp/releases/download/b11405/{}", f.name));

        let g = &HY_MT2_1_8B.files[0];
        assert_eq!(
            HY_MT2_1_8B.url("http://mirror.example/", g),
            format!("http://mirror.example/{}/resolve/{}/{}", HY_MT2_1_8B.repo, HY_MT2_1_8B.commit, g.name)
        );
    }

    fn make_zip(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(body).unwrap();
        }
        zw.finish().unwrap();
    }

    const PKG_SPEC: ModelSpec = ModelSpec {
        id: "pkg-test",
        lang: Lang::Unknown,
        label: "test",
        dir_name: "pkg",
        repo: "x/y",
        commit: "c",
        license: "MIT",
        license_url: "http://x",
        files: &[FileSpec { name: "a.bin", size: 3, sha256: "" }],
        url_base: None,
        unzip: false,
        ready_marker: Some("llama-server.exe"),
    };

    // T16:zip 解到 dest 並保留子資料夾
    #[test]
    fn t16_extract_zip_extracts_files_and_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let zp = tmp.path().join("p.zip");
        make_zip(&zp, &[("llama-server.exe", b"exe"), ("sub/dir/a.dll", b"dll")]);
        let dest = tmp.path().join("out");
        extract_zip(&zp, &dest).expect("解壓應成功");
        let (exe, dll) = (dest.join("llama-server.exe"), dest.join("sub").join("dir").join("a.dll"));
        assert!(exe.is_file(), "應解出 llama-server.exe");
        assert!(dll.is_file(), "應解出子資料夾內的 a.dll");
        assert_eq!(std::fs::read(&exe).unwrap_or_default(), b"exe");
        assert_eq!(std::fs::read(&dll).unwrap_or_default(), b"dll");
    }

    // T16:zip 內含 ../ 跳出 dest 的路徑 → Err,dest 外不出現檔案
    #[test]
    fn t16_extract_zip_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let zp = tmp.path().join("evil.zip");
        make_zip(&zp, &[("../evil.txt", b"x")]);
        let dest = tmp.path().join("out");
        let r = extract_zip(&zp, &dest);
        assert!(r.is_err(), "含 ../ 的 zip 應回 Err,實際 {r:?}");
        assert!(!tmp.path().join("evil.txt").exists(), "dest 外不該出現檔案");
    }

    // T16:files 大小正確但缺 marker → Partial 並列出 marker;補上 marker → Ready
    #[test]
    fn t16_status_requires_ready_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pkg");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), b"abc").unwrap();
        match model_status(&PKG_SPEC, tmp.path()) {
            ModelStatus::Partial { missing } => {
                assert!(missing.iter().any(|m| m == "llama-server.exe"), "missing 應含 marker,實際 {missing:?}")
            }
            other => panic!("缺 marker 應為 Partial,實際 {other:?}"),
        }
        std::fs::write(dir.join("llama-server.exe"), b"exe").unwrap();
        assert_eq!(model_status(&PKG_SPEC, tmp.path()), ModelStatus::Ready);
    }

    fn touch_file(p: &std::path::Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    // T24:腳本布局與 app 內布局都算可用;什麼都沒有或只有空資料夾為 false
    #[test]
    fn t24_package_usable_accepts_script_and_app_layouts() {
        // 腳本布局:zip 在 bin 根層、exe 在 bin/llama、gguf 在 models 根層
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        touch_file(&d.join("bin").join("llama-b11405-bin-win-vulkan-x64.zip"));
        touch_file(&d.join("bin").join("llama").join("llama-server.exe"));
        touch_file(&d.join("models").join("Hy-MT2-1.8B-Q8_0.gguf"));
        assert!(package_usable(&LLAMA_CPP, d), "腳本布局的 llama.cpp 應可用");
        assert!(package_usable(&HY_MT2_1_8B, d), "腳本布局的翻譯模型應可用");

        // app 內布局:zip 與 exe 在 bin/llama、gguf 在 models/hy-mt2-1.8b
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        touch_file(&d.join("bin").join("llama").join("llama-b11405-bin-win-vulkan-x64.zip"));
        touch_file(&d.join("bin").join("llama").join("llama-server.exe"));
        touch_file(&d.join("models").join("hy-mt2-1.8b").join("Hy-MT2-1.8B-Q8_0.gguf"));
        assert!(package_usable(&LLAMA_CPP, d), "app 布局的 llama.cpp 應可用");
        assert!(package_usable(&HY_MT2_1_8B, d), "app 布局的翻譯模型應可用");

        // 什麼都沒有
        let tmp = tempfile::tempdir().unwrap();
        assert!(!package_usable(&LLAMA_CPP, tmp.path()));
        assert!(!package_usable(&HY_MT2_1_8B, tmp.path()));
        assert!(!package_usable(&REAZONSPEECH, tmp.path()));

        // 只有 bin/llama 資料夾、沒有 exe
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("bin").join("llama")).unwrap();
        assert!(!package_usable(&LLAMA_CPP, tmp.path()), "沒有 llama-server.exe 不算可用");
    }
}
