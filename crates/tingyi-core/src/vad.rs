//! Silero VAD 切段(透過 sherpa-onnx)。
//!
//! 規則(規格書):連續靜音 ≥ 500 ms 視為段落結束;單段上限 8 秒;段首段尾各補 200 ms。

use anyhow::{anyhow, Result};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use crate::audio::TARGET_SAMPLE_RATE;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub model: PathBuf,
    pub threshold: f32,
    pub min_silence_sec: f32,
    pub min_speech_sec: f32,
    pub max_speech_sec: f32,
    /// Silero 視窗(16 kHz 時為 512)。
    pub window_size: i32,
    /// 段首段尾補的緩衝(毫秒)。
    pub pad_ms: u32,
    pub num_threads: i32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            model: crate::paths::silero_vad_model(),
            threshold: 0.5,
            min_silence_sec: 0.5,
            min_speech_sec: 0.25,
            max_speech_sec: 8.0,
            window_size: 512,
            pad_ms: 200,
            num_threads: 1,
        }
    }
}

/// 一個語音段(16 kHz mono,已含前後緩衝)。
#[derive(Debug, Clone)]
pub struct Segment {
    pub samples: Vec<f32>,
    /// 段首在整條串流中的樣本索引(含緩衝)。
    pub start_sample: i64,
    /// 段尾被 VAD 判定的瞬間(端到端延遲原點)。
    pub end_at: Instant,
}

impl Segment {
    pub fn duration_ms(&self) -> u32 {
        (self.samples.len() as f32 * 1000.0 / TARGET_SAMPLE_RATE as f32) as u32
    }
}

/// 把連續 16 kHz 音訊切成語音段。
pub struct Segmenter {
    vad: VoiceActivityDetector,
    pad_samples: usize,
    /// 已餵進 VAD 的總樣本數(VAD 回傳的 start 以此計)。
    fed: i64,
    /// 最近的原始樣本,供補前後緩衝用。
    history: VecDeque<f32>,
    history_start: i64,
    history_cap: usize,
}

impl Segmenter {
    pub fn new(cfg: &VadConfig) -> Result<Self> {
        let model = cfg.model.to_string_lossy().to_string();
        if !cfg.model.exists() {
            return Err(anyhow!("找不到 Silero VAD 模型:{model}"));
        }
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model),
                threshold: cfg.threshold,
                min_silence_duration: cfg.min_silence_sec,
                min_speech_duration: cfg.min_speech_sec,
                window_size: cfg.window_size,
                max_speech_duration: cfg.max_speech_sec,
            },
            sample_rate: TARGET_SAMPLE_RATE,
            num_threads: cfg.num_threads,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        };
        // VAD 內部緩衝 60 秒,足以容納 max_speech + 靜音。
        let vad = VoiceActivityDetector::create(&config, 60.0)
            .ok_or_else(|| anyhow!("VoiceActivityDetector::create 失敗"))?;
        let pad_samples = (cfg.pad_ms as usize * TARGET_SAMPLE_RATE as usize) / 1000;
        let history_cap = (cfg.max_speech_sec as usize + 4) * TARGET_SAMPLE_RATE as usize;
        Ok(Self {
            vad,
            pad_samples,
            fed: 0,
            history: VecDeque::with_capacity(history_cap),
            history_start: 0,
            history_cap,
        })
    }

    /// 餵入樣本,回傳這次新切出的段落(可能 0 個或多個)。
    pub fn push(&mut self, samples: &[f32]) -> Vec<Segment> {
        self.vad.accept_waveform(samples);
        self.fed += samples.len() as i64;
        self.history.extend(samples.iter().copied());
        while self.history.len() > self.history_cap {
            self.history.pop_front();
            self.history_start += 1;
        }
        self.drain(Instant::now())
    }

    /// 是否正在講話中(供即時預覽判斷)。
    pub fn speaking(&self) -> bool {
        self.vad.detected()
    }

    /// 強制結束目前段落(停止擷取時呼叫)。
    pub fn flush(&mut self) -> Vec<Segment> {
        self.vad.flush();
        self.drain(Instant::now())
    }

    fn drain(&mut self, now: Instant) -> Vec<Segment> {
        let mut out = Vec::new();
        while !self.vad.is_empty() {
            if let Some(seg) = self.vad.front() {
                let start = seg.start() as i64;
                let n = seg.n() as i64;
                out.push(self.with_padding(start, n, seg.samples(), now));
            }
            self.vad.pop();
        }
        out
    }

    fn with_padding(&self, start: i64, n: i64, core: &[f32], now: Instant) -> Segment {
        let pad = self.pad_samples as i64;
        let want_start = (start - pad).max(self.history_start);
        let want_end = (start + n + pad).min(self.history_start + self.history.len() as i64);
        // history 可能已不含這段(極端情況),就退回 VAD 給的原始樣本。
        if want_start >= want_end || start < self.history_start {
            return Segment {
                samples: core.to_vec(),
                start_sample: start,
                end_at: now,
            };
        }
        let a = (want_start - self.history_start) as usize;
        let b = (want_end - self.history_start) as usize;
        let samples: Vec<f32> = self.history.range(a..b).copied().collect();
        Segment {
            samples,
            start_sample: want_start,
            end_at: now,
        }
    }
}

/// 斷句合併判斷(規格書):段落很短且無句末標點 → 等下一段合併。
pub fn is_fragment(text: &str, lang_is_cjk: bool) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if ends_with_terminal_punct(t) {
        return false;
    }
    if lang_is_cjk {
        t.chars().count() < 6
    } else {
        t.split_whitespace().count() < 4
    }
}

pub fn ends_with_terminal_punct(t: &str) -> bool {
    matches!(
        t.chars().last(),
        Some('.' | '!' | '?' | '。' | '\u{FF01}' | '\u{FF1F}' | '…' | '」' | '』')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_rules() {
        assert!(is_fragment("and then", false));
        assert!(!is_fragment("and then we go", false));
        assert!(!is_fragment("ok.", false));
        assert!(is_fragment("それで", true));
        assert!(!is_fragment("それでは始めましょう", true));
        assert!(!is_fragment("そう?", true));
        assert!(!is_fragment("", true));
    }
}
