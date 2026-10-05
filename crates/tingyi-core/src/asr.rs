//! 語音辨識:統一介面 `Transcriber` + SenseVoice-Small(sherpa-onnx,CPU)。
//!
//! 語言限定英、日、韓;判定為中文時原文照顯示、不翻譯。
//! 注意:sherpa-onnx 的 Rust 包裝不回傳 SenseVoice 的 lang 欄位,
//! 這裡以字集啟發式判斷語言(假名 → ja、諺文 → ko、拉丁 → en、純漢字 → zh)。

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};
use std::path::PathBuf;
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
        let stream = self.recognizer.create_stream();
        if let Some(l) = lang_hint.or(self.locked) {
            if stream.has_option("language") {
                stream.set_option("language", l.code());
            }
        }
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

fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x30FF   // CJK 標點、平假名、片假名
        | 0x31F0..=0x31FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF // 漢字
        | 0xAC00..=0xD7AF // 諺文
        | 0xFF00..=0xFFEF // 全形字元
    )
}

/// 移除 CJK 字元之間的空白。
///
/// SenseVoice 對語速快的日文常輸出「クレン ゲーム に 行っ た」這種以空白斷開的 token,
/// 翻譯模型吃到會明顯變差;拉丁字母之間的空白(英文單字)保留。
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
        assert_eq!(collapse_cjk_spaces("안녕 하세요 OK then"), "안녕하세요OK then");
        assert_eq!(strip_tags("<|ja|> 今日 は いい 天気"), "今日はいい天気");
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
}
