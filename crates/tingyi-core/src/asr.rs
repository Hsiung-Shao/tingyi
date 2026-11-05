//! 語音辨識:統一介面 `Transcriber` + SenseVoice-Small(sherpa-onnx,CPU)。
//!
//! 語言限定英、日、韓;判定為中文時原文照顯示、不翻譯。
//! 注意:sherpa-onnx 的 Rust 包裝不回傳 SenseVoice 的 lang 欄位,
//! 這裡以字集啟發式判斷語言(假名 → ja、諺文 → ko、拉丁 → en、純漢字 → zh)。

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, OfflineTransducerModelConfig};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::audio::TARGET_SAMPLE_RATE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    En,
    Ja,
    Ko,
    Zh,
    Unknown,
}

impl Default for Lang {
    fn default() -> Self {
        Lang::Unknown
    }
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ja => "ja",
            Lang::Ko => "ko",
            Lang::Zh => "zh",
            Lang::Unknown => "auto",
        }
    }

    pub fn is_cjk(self) -> bool {
        matches!(self, Lang::Ja | Lang::Ko | Lang::Zh)
    }

    /// 送翻譯時的來源語言名稱(Hy-MT2 提示用中文)。
    pub fn zh_name(self) -> &'static str {
        match self {
            Lang::En => "英文",
            Lang::Ja => "日文",
            Lang::Ko => "韓文",
            Lang::Zh => "中文",
            Lang::Unknown => "原文",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    /// 已剝除標籤、trim 後的文字。
    pub text: String,
    pub lang: Lang,
    /// 模型原始輸出(含標籤)。
    pub raw: String,
    pub asr_ms: u32,
}

/// 辨識引擎統一介面,方便日後換 Whisper / Qwen3-ASR / 雲端。
pub trait Transcriber: Send {
    /// `pcm` 為 16 kHz 單聲道;`lang_hint` 為使用者鎖定的語言。
    fn transcribe(&self, pcm: &[f32], lang_hint: Option<Lang>) -> Result<Transcript>;
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct SenseVoiceConfig {
    pub model: PathBuf,
    pub tokens: PathBuf,
    pub num_threads: i32,
    /// 逆文本正規化(數字、日期轉寫)。
    pub use_itn: bool,
    /// 建立時鎖定語言;None = auto。
    pub language: Option<Lang>,
}

impl Default for SenseVoiceConfig {
    fn default() -> Self {
        Self {
            model: crate::paths::sensevoice_model(),
            tokens: crate::paths::sensevoice_tokens(),
            num_threads: 4,
            use_itn: true,
            language: None,
        }
    }
}

pub struct SenseVoice {
    recognizer: OfflineRecognizer,
    locked: Option<Lang>,
}

impl SenseVoice {
    pub fn new(cfg: &SenseVoiceConfig) -> Result<Self> {
        for p in [&cfg.model, &cfg.tokens] {
            if !p.exists() {
                return Err(anyhow!("找不到 SenseVoice 檔案:{}", p.display()));
            }
        }
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(cfg.model.to_string_lossy().to_string()),
            language: Some(cfg.language.map(|l| l.code()).unwrap_or("auto").to_string()),
            use_itn: cfg.use_itn,
        };
        config.model_config.tokens = Some(cfg.tokens.to_string_lossy().to_string());
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some("cpu".into());
        config.model_config.debug = false;
        config.decoding_method = Some("greedy_search".into());
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("OfflineRecognizer::create(SenseVoice) 失敗"))?;
        Ok(Self {
            recognizer,
            locked: cfg.language,
        })
    }
}

impl Transcriber for SenseVoice {
    fn transcribe(&self, pcm: &[f32], lang_hint: Option<Lang>) -> Result<Transcript> {
        let t0 = Instant::now();
        // 語言只能在建立辨識器時鎖定(sherpa-onnx 的 SenseVoice 不讀 stream 的 language 選項),
        // `lang_hint` 只用來標記輸出語言。
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(TARGET_SAMPLE_RATE, pcm);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("SenseVoice 無結果"))?;
        let raw = result.text;
        let text = strip_tags(&raw);
        let lang = lang_hint
            .or(self.locked)
            .unwrap_or_else(|| detect_lang(&text));
        Ok(Transcript {
            text,
            lang,
            raw,
            asr_ms: t0.elapsed().as_millis() as u32,
        })
    }

    fn name(&self) -> &'static str {
        "sensevoice-small-int8"
    }
}

// ---------------------------------------------------------------------------
// 單一語言的 transducer 專用模型(ReazonSpeech zipformer、NeMo Parakeet TDT;sherpa-onnx,CPU)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TransducerConfig {
    /// 模型目錄;encoder / decoder / joiner 優先取 `*.int8.onnx`。
    pub dir: PathBuf,
    /// sherpa-onnx 的 model_type:Parakeet 要 `nemo_transducer`,zipformer 留 None。
    pub model_type: Option<String>,
    pub num_threads: i32,
    /// 模型的語言;輸出一律標成這個語言(或呼叫端鎖定的語言)。
    pub lang: Lang,
    /// 段首段尾墊的合成靜音(毫秒):段首直接是語音時 ReazonSpeech 會吞第一個詞(hayamimi 實測)。
    pub pad_ms: u32,
    pub name: &'static str,
}

impl TransducerConfig {
    pub fn reazonspeech(dir_name: &str, num_threads: i32) -> Self {
        Self {
            dir: crate::paths::models_dir().join(dir_name),
            model_type: None,
            num_threads,
            lang: Lang::Ja,
            pad_ms: 0,
            name: if dir_name == crate::paths::REAZONSPEECH_JA_DIR_NAME { "reazonspeech-ja-int8" } else { "reazonspeech-ja-en-int8" },
        }
    }

    pub fn parakeet(num_threads: i32) -> Self {
        Self {
            dir: crate::paths::models_dir().join(crate::paths::PARAKEET_DIR_NAME),
            model_type: Some("nemo_transducer".into()),
            num_threads,
            lang: Lang::En,
            pad_ms: 0,
            name: "parakeet-tdt-0.6b-v2-int8",
        }
    }
}

/// 在模型目錄找 `<prefix>*.int8.onnx`,沒有再找 `<prefix>*.onnx`(不取 fp16)。
fn find_onnx(dir: &Path, prefix: &str) -> Result<PathBuf> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| anyhow!("讀不到模型目錄 {}:{e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with(prefix) && n.ends_with(".onnx") && !n.ends_with(".fp16.onnx"))
        .collect();
    names
        .iter()
        .find(|n| n.ends_with(".int8.onnx"))
        .or_else(|| names.first())
        .map(|n| dir.join(n))
        .ok_or_else(|| anyhow!("{} 裡找不到 {prefix}*.onnx", dir.display()))
}

pub struct SherpaTransducer {
    recognizer: OfflineRecognizer,
    lang: Lang,
    pad: usize,
    name: &'static str,
}

impl SherpaTransducer {
    pub fn new(cfg: &TransducerConfig) -> Result<Self> {
        let tokens = cfg.dir.join("tokens.txt");
        if !tokens.exists() {
            return Err(anyhow!("找不到 {}", tokens.display()));
        }
        let path = |p: PathBuf| Some(p.to_string_lossy().to_string());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: path(find_onnx(&cfg.dir, "encoder")?),
            decoder: path(find_onnx(&cfg.dir, "decoder")?),
            joiner: path(find_onnx(&cfg.dir, "joiner")?),
        };
        config.model_config.tokens = path(tokens);
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some("cpu".into());
        config.model_config.model_type = cfg.model_type.clone();
        config.decoding_method = Some("greedy_search".into());
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("OfflineRecognizer::create({}) 失敗", cfg.name))?;
        Ok(Self {
            recognizer,
            lang: cfg.lang,
            pad: cfg.pad_ms as usize * TARGET_SAMPLE_RATE as usize / 1000,
            name: cfg.name,
        })
    }
}

impl Transcriber for SherpaTransducer {
    fn transcribe(&self, pcm: &[f32], lang_hint: Option<Lang>) -> Result<Transcript> {
        let t0 = Instant::now();
        let stream = self.recognizer.create_stream();
        if self.pad > 0 {
            let mut padded = vec![0.0f32; self.pad];
            padded.extend_from_slice(pcm);
            padded.resize(padded.len() + self.pad, 0.0);
            stream.accept_waveform(TARGET_SAMPLE_RATE, &padded);
        } else {
            stream.accept_waveform(TARGET_SAMPLE_RATE, pcm);
        }
        self.recognizer.decode(&stream);
        let raw = stream
            .get_result()
            .ok_or_else(|| anyhow!("{} 無結果", self.name))?
            .text;
        Ok(Transcript {
            text: collapse_cjk_spaces(raw.trim()),
            lang: lang_hint.unwrap_or(self.lang),
            raw,
            asr_ms: t0.elapsed().as_millis() as u32,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

// ---------------------------------------------------------------------------
// Qwen3-ASR、Cohere Transcribe(sherpa-onnx,CPU;A/B 量測用,app 尚未使用)
// ---------------------------------------------------------------------------

/// 剝掉 Qwen3-ASR 的 `language X<asr_text>` 前綴(sherpa-onnx 通常已剝,這裡是備援);
/// 有前綴就用它標的語言,沒有就以字集判斷。
pub fn parse_qwen3_output(raw: &str) -> (String, Lang) {
    // 實測 sherpa-onnx 1.13.8 不一定停在 <|endoftext|>,後面會接著亂生一段:截斷
    let end = ["<|endoftext|>", "<|im_end|>"].iter().filter_map(|t| raw.find(t)).min().unwrap_or(raw.len());
    let s = raw[..end].trim();
    if let Some(rest) = s.strip_prefix("language ") {
        if let Some((name, text)) = rest.split_once("<asr_text>") {
            let text = text.trim().to_string();
            let lang = match name.trim() {
                "English" => Lang::En,
                "Japanese" => Lang::Ja,
                "Korean" => Lang::Ko,
                "Chinese" | "Cantonese" => Lang::Zh,
                _ => detect_lang(&text),
            };
            return (text, lang);
        }
    }
    (s.to_string(), detect_lang(s))
}

fn require(dir: &Path, name: &str) -> Result<String> {
    let p = dir.join(name);
    if p.exists() {
        Ok(p.to_string_lossy().to_string())
    } else {
        Err(anyhow!("找不到 {}", p.display()))
    }
}

#[derive(Debug, Clone)]
pub struct Qwen3Config {
    /// 內含 conv_frontend.onnx、encoder*.onnx、decoder*.onnx。
    pub dir: PathBuf,
    /// tokenizer 目錄(0.6B 在 dir/tokenizer;ModelScope 的 1.7B 在上一層)。
    pub tokenizer: PathBuf,
    pub num_threads: i32,
    pub name: &'static str,
}

pub struct SherpaQwen3 {
    recognizer: OfflineRecognizer,
    name: &'static str,
}

impl SherpaQwen3 {
    pub fn new(cfg: &Qwen3Config) -> Result<Self> {
        let path = |p: PathBuf| Some(p.to_string_lossy().to_string());
        let encoder = path(find_onnx(&cfg.dir, "encoder")?);
        let decoder = path(find_onnx(&cfg.dir, "decoder")?);
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.qwen3_asr = sherpa_onnx::OfflineQwen3ASRModelConfig {
            conv_frontend: Some(require(&cfg.dir, "conv_frontend.onnx")?),
            encoder,
            decoder,
            tokenizer: Some(if cfg.tokenizer.exists() {
                cfg.tokenizer.to_string_lossy().to_string()
            } else {
                return Err(anyhow!("找不到 tokenizer 目錄 {}", cfg.tokenizer.display()));
            }),
            max_new_tokens: 256,
            ..Default::default()
        };
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some("cpu".into());
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("OfflineRecognizer::create({}) 失敗", cfg.name))?;
        Ok(Self { recognizer, name: cfg.name })
    }
}

impl Transcriber for SherpaQwen3 {
    fn transcribe(&self, pcm: &[f32], lang_hint: Option<Lang>) -> Result<Transcript> {
        let t0 = Instant::now();
        let stream = self.recognizer.create_stream();
        // Qwen3-ASR 讀 stream 的 language 選項(提示詞裡放 `language X<asr_text>`)
        let name = match lang_hint {
            Some(Lang::En) => Some("English"),
            Some(Lang::Ja) => Some("Japanese"),
            Some(Lang::Ko) => Some("Korean"),
            Some(Lang::Zh) => Some("Chinese"),
            _ => None,
        };
        if let Some(n) = name {
            stream.set_option("language", n);
        }
        stream.accept_waveform(TARGET_SAMPLE_RATE, pcm);
        self.recognizer.decode(&stream);
        let raw = stream.get_result().ok_or_else(|| anyhow!("{} 無結果", self.name))?.text;
        let (text, lang) = parse_qwen3_output(&raw);
        Ok(Transcript {
            text: collapse_cjk_spaces(&text),
            lang: lang_hint.unwrap_or(lang),
            raw,
            asr_ms: t0.elapsed().as_millis() as u32,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

#[derive(Debug, Clone)]
pub struct CohereConfig {
    pub dir: PathBuf,
    pub num_threads: i32,
    /// Cohere Transcribe 要在建立時指定語言。
    pub lang: Lang,
    pub name: &'static str,
}

pub struct SherpaCohere {
    recognizer: OfflineRecognizer,
    lang: Lang,
    name: &'static str,
}

impl SherpaCohere {
    pub fn new(cfg: &CohereConfig) -> Result<Self> {
        let path = |p: PathBuf| Some(p.to_string_lossy().to_string());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.cohere_transcribe = sherpa_onnx::OfflineCohereTranscribeModelConfig {
            encoder: path(find_onnx(&cfg.dir, "encoder")?),
            decoder: path(find_onnx(&cfg.dir, "decoder")?),
            language: Some(cfg.lang.code().to_string()),
            use_punct: true,
            use_itn: true,
        };
        config.model_config.tokens = Some(require(&cfg.dir, "tokens.txt")?);
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some("cpu".into());
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("OfflineRecognizer::create({}) 失敗", cfg.name))?;
        Ok(Self { recognizer, lang: cfg.lang, name: cfg.name })
    }
}

impl Transcriber for SherpaCohere {
    fn transcribe(&self, pcm: &[f32], _lang_hint: Option<Lang>) -> Result<Transcript> {
        let t0 = Instant::now();
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(TARGET_SAMPLE_RATE, pcm);
        self.recognizer.decode(&stream);
        let raw = stream.get_result().ok_or_else(|| anyhow!("{} 無結果", self.name))?.text;
        Ok(Transcript {
            text: collapse_cjk_spaces(raw.trim()),
            // 實際是用建立時的語言解碼,標記也照實
            lang: self.lang,
            raw,
            asr_ms: t0.elapsed().as_millis() as u32,
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

// ---------------------------------------------------------------------------
// Whisper(whisper.cpp whisper-server,GPU)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WhisperServerConfig {
    /// 例如 `http://127.0.0.1:8179`
    pub base_url: String,
    /// 鎖定語言;None = 自動偵測(限 en/ja/ko,由字集啟發式覆核)。
    pub language: Option<Lang>,
    pub timeout: std::time::Duration,
}

impl Default for WhisperServerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8179".into(),
            language: None,
            timeout: std::time::Duration::from_secs(10),
        }
    }
}

/// 透過 whisper-server `/inference` 辨識。即時預覽不用它(每秒一次太重),只做段落正式辨識。
pub struct WhisperServer {
    cfg: WhisperServerConfig,
    agent: ureq::Agent,
}

/// Whisper 對雜音/靜音常幻聽出的固定句,整句命中就視為無文字。
const WHISPER_HALLUCINATIONS: &[&str] = &[
    "ご視聴ありがとうございました",
    "ご視聴ありがとうございました。",
    "チャンネル登録お願いします",
    "字幕 by",
    "Thank you for watching",
    "Thanks for watching",
    "Subtitles by the Amara.org community",
    "시청해 주셔서 감사합니다",
    "謝謝觀看",
];

impl WhisperServer {
    pub fn new(cfg: WhisperServerConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(cfg.timeout)
            .build();
        Self { cfg, agent }
    }

    fn lang_param(&self, hint: Option<Lang>) -> String {
        hint.or(self.cfg.language)
            .filter(|l| *l != Lang::Unknown)
            .map(|l| l.code().to_string())
            .unwrap_or_else(|| "auto".into())
    }
}

#[derive(serde::Deserialize)]
struct WhisperResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    language: Option<String>,
}

/// `verbose_json` 回應(回修用):segments 帶秒數時間戳。
#[derive(serde::Deserialize)]
struct WhisperVerboseResponse {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    segments: Vec<WhisperSegmentJson>,
}

#[derive(serde::Deserialize)]
struct WhisperSegmentJson {
    #[serde(default)]
    start: f32,
    #[serde(default)]
    end: f32,
    #[serde(default)]
    text: String,
}

/// 有時間戳的一段(秒,相對送入音訊的起點)。
#[derive(Debug, Clone)]
pub struct TimedSegment {
    pub start_s: f32,
    pub end_s: f32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TimedTranscript {
    pub segments: Vec<TimedSegment>,
    pub lang: Lang,
    pub asr_ms: u32,
}

/// Whisper 對雜音/靜音常幻聽出的固定句:整句 trim 後比對(不分大小寫、可少句尾「。」)。
pub fn is_hallucination(text: &str) -> bool {
    let t = text.trim();
    WHISPER_HALLUCINATIONS
        .iter()
        .any(|h| t.eq_ignore_ascii_case(h) || t == h.trim_end_matches('。'))
}

fn parse_whisper_lang(s: Option<&str>, text: &str) -> Lang {
    match s {
        Some("en") | Some("english") => Lang::En,
        Some("ja") | Some("japanese") => Lang::Ja,
        Some("ko") | Some("korean") => Lang::Ko,
        Some("zh") | Some("yue") | Some("chinese") | Some("cantonese") => Lang::Zh,
        _ => detect_lang(text),
    }
}

fn parse_verbose(parsed: WhisperVerboseResponse, asr_ms: u32, lang_hint: Option<Lang>) -> TimedTranscript {
    let segments: Vec<TimedSegment> = parsed
        .segments
        .into_iter()
        .map(|s| TimedSegment {
            start_s: s.start,
            end_s: s.end,
            text: strip_tags(&s.text.replace(['\r', '\n'], " ")),
        })
        .filter(|s| !s.text.is_empty() && !is_hallucination(&s.text))
        .collect();
    let all: String = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
    let lang = lang_hint.unwrap_or_else(|| parse_whisper_lang(parsed.language.as_deref(), &all));
    TimedTranscript { segments, lang, asr_ms }
}

impl WhisperServer {
    /// 有時間戳的整段辨識(回修用):`verbose_json`、關 token 時間戳與語言機率(省時間)、
    /// 壓掉非語音 token、`prompt` 帶上一視窗末句當前文提示。
    pub fn transcribe_timestamped(&self, pcm: &[f32], lang_hint: Option<Lang>, prompt: Option<&str>) -> Result<TimedTranscript> {
        let t0 = Instant::now();
        let lang = self.lang_param(lang_hint);
        let wav = pcm_to_wav16(pcm, TARGET_SAMPLE_RATE as u32);
        let mut fields: Vec<(&str, String)> = vec![
            ("language", lang),
            ("response_format", "verbose_json".into()),
            ("no_timestamps", "false".into()),
            ("token_timestamps", "false".into()),
            ("no_language_probabilities", "true".into()),
            ("suppress_non_speech", "true".into()),
            ("temperature", "0.0".into()),
            ("no_speech_thold", "0.6".into()),
        ];
        if let Some(p) = prompt.map(str::trim).filter(|p| !p.is_empty()) {
            let p: String = p.chars().take(100).collect();
            fields.push(("prompt", p));
        }
        let boundary = "----tingyi-boundary-7d1e9c0f";
        let body = build_multipart(boundary, &fields, "file", "win.wav", "audio/wav", &wav);
        let url = format!("{}/inference", self.cfg.base_url.trim_end_matches('/'));
        let resp = self
            .agent
            .post(&url)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send_bytes(&body)
            .map_err(|e| anyhow!("whisper-server /inference(verbose): {e}"))?;
        let parsed: WhisperVerboseResponse = resp
            .into_json()
            .map_err(|e| anyhow!("whisper-server verbose_json 解析失敗: {e}"))?;
        Ok(parse_verbose(parsed, t0.elapsed().as_millis() as u32, lang_hint.or(self.cfg.language)))
    }
}

impl Transcriber for WhisperServer {
    fn transcribe(&self, pcm: &[f32], lang_hint: Option<Lang>) -> Result<Transcript> {
        let t0 = Instant::now();
        let lang = self.lang_param(lang_hint);
        let wav = pcm_to_wav16(pcm, TARGET_SAMPLE_RATE as u32);
        // 注意:不可送 detect_language=true,那會讓 whisper-server 只偵測語言、回空文字;
        // language=auto 本身就會自動偵測並辨識。
        let fields: Vec<(&str, String)> = vec![
            ("language", lang.clone()),
            ("response_format", "json".into()),
            // 不可開 no_timestamps:無時間戳模式遇到句中停頓會提早收尾,
            // 4 秒的段落只剩前面一兩個詞(2026-10-06 日文直播錄音實測)。
            ("no_timestamps", "false".into()),
            ("temperature", "0.0".into()),
            ("no_speech_thold", "0.6".into()),
        ];
        let boundary = "----tingyi-boundary-7d1e9c0f";
        let body = build_multipart(boundary, &fields, "file", "seg.wav", "audio/wav", &wav);
        let url = format!("{}/inference", self.cfg.base_url.trim_end_matches('/'));
        let resp = self
            .agent
            .post(&url)
            .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .send_bytes(&body)
            .map_err(|e| anyhow!("whisper-server /inference: {e}"))?;
        let parsed: WhisperResponse = resp
            .into_json()
            .map_err(|e| anyhow!("whisper-server 回應不是 JSON: {e}"))?;
        let raw = parsed.text;
        // 有時間戳模式會以換行分段;併成一行再做後處理
        let joined = raw.replace(['\r', '\n'], " ");
        let mut text = strip_tags(&joined);
        if is_hallucination(&text) {
            text.clear();
        }
        let detected = lang_hint
            .or(self.cfg.language)
            .unwrap_or_else(|| parse_whisper_lang(parsed.language.as_deref(), &text));
        Ok(Transcript {
            text,
            lang: detected,
            raw,
            asr_ms: t0.elapsed().as_millis() as u32,
        })
    }

    fn name(&self) -> &'static str {
        "whisper-large-v3-turbo"
    }
}

/// f32 PCM → 16-bit PCM WAV(44 byte header)。
pub fn pcm_to_wav16(pcm: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &x in pcm {
        let v = (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// 手組 multipart/form-data(文字欄位 + 一個檔案)。
pub fn build_multipart(
    boundary: &str,
    fields: &[(&str, String)],
    file_field: &str,
    filename: &str,
    mime: &str,
    file_bytes: &[u8],
) -> Vec<u8> {
    let mut b = Vec::with_capacity(file_bytes.len() + 512);
    for (k, v) in fields {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    b.extend_from_slice(file_bytes);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

/// 剝除 SenseVoice 的 `<|xx|>` 標籤(語言、情緒、音效、itn)並整理空白。
pub fn strip_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("<|") {
        out.push_str(&rest[..start]);
        match rest[start..].find("|>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let joined = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapse_cjk_spaces(&joined)
}

/// 中日文字元(不含諺文:韓文本來就以空白分詞,空白要保留)。
fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x30FF   // CJK 標點、平假名、片假名
        | 0x31F0..=0x31FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF // 漢字
        | 0xFF00..=0xFFEF // 全形字元
    )
}

/// 移除中日文字元之間的空白(韓文保留)。
///
/// SenseVoice 對語速快的日文常輸出「クレン ゲーム に 行っ た」這種以空白斷開的 token,
/// 翻譯模型吃到會明顯變差;拉丁字母之間的空白(英文單字)與韓文分詞空白保留。
pub fn collapse_cjk_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let prev = chars[..i].iter().rev().find(|x| **x != ' ').copied();
            let next = chars[i + 1..].iter().find(|x| **x != ' ').copied();
            if let (Some(p), Some(n)) = (prev, next) {
                if is_cjk_char(p) || is_cjk_char(n) {
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// 字集啟發式語言判斷。
pub fn detect_lang(text: &str) -> Lang {
    let mut kana = 0usize;
    let mut hangul = 0usize;
    let mut han = 0usize;
    let mut latin = 0usize;
    for c in text.chars() {
        let u = c as u32;
        match u {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF => han += 1,
            _ if c.is_ascii_alphabetic() => latin += 1,
            _ => {}
        }
    }
    if kana > 0 && kana * 10 >= (kana + han + latin) {
        return Lang::Ja;
    }
    if hangul > 0 && hangul >= han && hangul >= latin / 2 {
        return Lang::Ko;
    }
    if han > 0 && han >= latin {
        return Lang::Zh;
    }
    if latin > 0 {
        return Lang::En;
    }
    Lang::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sensevoice_tags() {
        assert_eq!(
            strip_tags("<|ja|><|NEUTRAL|><|Speech|><|woitn|>今日は いい天気ですね"),
            "今日はいい天気ですね"
        );
        assert_eq!(strip_tags("<|en|><|HAPPY|><|Laughter|>hello  there"), "hello there");
        assert_eq!(strip_tags("<|BGM|>"), "");
        assert_eq!(strip_tags("no tags"), "no tags");
    }

    #[test]
    fn collapses_spaces_between_cjk_only() {
        assert_eq!(collapse_cjk_spaces("クレン ゲーム に 行っ た 話 とか"), "クレンゲームに行った話とか");
        assert_eq!(collapse_cjk_spaces("2000 円 ぐらい で 2 個"), "2000円ぐらいで2個");
        assert_eq!(collapse_cjk_spaces("hello there world"), "hello there world");
        assert_eq!(collapse_cjk_spaces("Twitter で 見た"), "Twitterで見た");
        assert_eq!(collapse_cjk_spaces("안녕 하세요 OK then"), "안녕 하세요 OK then"); // 韓文空白保留
        assert_eq!(strip_tags("<|ja|> 今日 は いい 天気"), "今日はいい天気");
    }

    #[test]
    fn wav_header_and_multipart() {
        let wav = pcm_to_wav16(&[0.0, 0.5, -0.5, 1.0], 16000);
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16000);
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 8);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), 16384); // 0.5
        assert_eq!(i16::from_le_bytes([wav[50], wav[51]]), 32767); // 1.0

        let body = build_multipart("B", &[("language", "ja".into())], "file", "a.wav", "audio/wav", b"XYZ");
        let s = String::from_utf8_lossy(&body);
        assert!(s.starts_with("--B\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nja\r\n"));
        assert!(s.contains("name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\nXYZ\r\n--B--\r\n"));
    }

    #[test]
    fn detects_scripts() {
        assert_eq!(detect_lang("今日はいい天気ですね"), Lang::Ja);
        assert_eq!(detect_lang("안녕하세요 여러분"), Lang::Ko);
        assert_eq!(detect_lang("Welcome back to the stream"), Lang::En);
        assert_eq!(detect_lang("今天天氣真好"), Lang::Zh);
        assert_eq!(detect_lang("OK"), Lang::En);
        assert_eq!(detect_lang("..."), Lang::Unknown);
        // 日文夾英文仍判日文
        assert_eq!(detect_lang("Twitterで見たんだけど"), Lang::Ja);
    }

    #[test]
    fn parse_verbose_json_segments() {
        let j = r#"{"task":"transcribe","language":"japanese","duration":4.0,"text":"a b",
            "segments":[{"id":0,"seek":0,"start":0.0,"end":1.5,"text":" 嘘でしょ?"},
                        {"id":1,"seek":0,"start":1.6,"end":3.9,"text":" ご視聴ありがとうございました"},
                        {"id":2,"seek":0,"start":3.9,"end":4.0,"text":"   "}]}"#;
        let parsed: WhisperVerboseResponse = serde_json::from_str(j).unwrap();
        let t = parse_verbose(parsed, 12, None);
        assert_eq!(t.lang, Lang::Ja);
        assert_eq!(t.segments.len(), 1, "幻聽與空白段要被濾掉");
        assert_eq!(t.segments[0].text, "嘘でしょ?");
        assert!((t.segments[0].end_s - 1.5).abs() < 1e-6);
        assert!(is_hallucination("Thanks for watching"));
        assert!(is_hallucination("ご視聴ありがとうございました。"));
        assert!(!is_hallucination("ありがとうございました"));
    }
}
