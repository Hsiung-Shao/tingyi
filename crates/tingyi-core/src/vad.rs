//! Silero VAD 切段(透過 sherpa-onnx)。
//!
//! 規則(規格書):連續靜音 ≥ 500 ms 視為段落結束;單段上限 8 秒;段首段尾各補 200 ms。

use anyhow::{anyhow, Result};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use crate::audio::TARGET_SAMPLE_RATE;

/// 沒有指定、也沒有引擎預設時的段首緩衝(毫秒)。
pub const DEFAULT_PRE_PAD_MS: u32 = 200;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub model: PathBuf,
    pub threshold: f32,
    pub min_silence_sec: f32,
    pub min_speech_sec: f32,
    pub max_speech_sec: f32,
    /// Silero 視窗(16 kHz 時為 512)。
    pub window_size: i32,
    /// 段首往前補的緩衝(毫秒);不會切進前一段的語音本體。None = 依辨識引擎的預設
    /// (`AsrEngine::default_pre_pad_ms`;直接用 Segmenter 時為 `DEFAULT_PRE_PAD_MS`)。
    pub pre_pad_ms: Option<u32>,
    /// 段尾補的緩衝(毫秒)。
    pub pad_ms: u32,
    pub num_threads: i32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            model: crate::paths::silero_vad_model(),
            // 2026-10-06 真實影片實測:0.5 s 靜音在連續對白中很少成立,段落常撞上限才切;
            // 改 0.35 s / 5 s 讓字幕更快出現(規格書原值 0.5 s / 8 s)。
            threshold: 0.5,
            min_silence_sec: 0.35,
            min_speech_sec: 0.25,
            max_speech_sec: 5.0,
            window_size: 512,
            pre_pad_ms: None,
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
    /// VAD 判定的語音本體 [start, end)(不含補白);斷句調整、說話者、字速都用這段。
    pub core: (i64, i64),
    /// 撞單段上限被強制切開(話還沒講完)。
    pub forced: bool,
}

impl Segment {
    pub fn duration_ms(&self) -> u32 {
        (self.samples.len() as f32 * 1000.0 / TARGET_SAMPLE_RATE as f32) as u32
    }

    pub fn core_ms(&self) -> u32 {
        ((self.core.1 - self.core.0).max(0) * 1000 / TARGET_SAMPLE_RATE as i64) as u32
    }

    /// 語音本體在 `samples` 中的索引範圍。
    pub fn core_range(&self) -> std::ops::Range<usize> {
        let len = self.samples.len() as i64;
        let a = (self.core.0 - self.start_sample).clamp(0, len);
        let b = (self.core.1 - self.start_sample).clamp(a, len);
        a as usize..b as usize
    }
}

/// 把連續 16 kHz 音訊切成語音段。
pub struct Segmenter {
    vad: VoiceActivityDetector,
    pre_pad: i64,
    post_pad: i64,
    /// 上一段語音本體的結尾(補白不切進前一段)。
    last_core_end: Option<i64>,
    /// 已餵進 Segmenter 的總樣本數。
    fed: i64,
    /// 目前這個 VAD 建立時的 `fed`:sherpa VAD 回傳的 start 從建立時起算,換參數重建後要加回原點。
    vad_origin: i64,
    /// 最近的原始樣本,供補前後緩衝用。
    history: VecDeque<f32>,
    history_start: i64,
    history_cap: usize,
    /// 目前正在講的這段語音的起點(樣本索引);None = 沒在講話。即時預覽用。
    speech_start: Option<i64>,
    /// 目前生效的設定(VAD 建立後不能改,要換參數得重建)。
    cfg: VadConfig,
    /// 等待套用的新參數(靜音秒數、單段上限秒數);在沒講話時重建 VAD 套用。
    pending: Option<(f32, f32)>,
}

impl Segmenter {
    pub fn new(cfg: &VadConfig) -> Result<Self> {
        let vad = Self::build_vad(cfg)?;
        let ms_to_samples = |ms: u32| ms as i64 * TARGET_SAMPLE_RATE as i64 / 1000;
        // history 要能容納最大的單段上限(自適應最高 6 s)+ 緩衝
        let history_cap = (cfg.max_speech_sec.max(6.0) as usize + 4) * TARGET_SAMPLE_RATE as usize;
        Ok(Self {
            vad,
            pre_pad: ms_to_samples(cfg.pre_pad_ms.unwrap_or(DEFAULT_PRE_PAD_MS)),
            post_pad: ms_to_samples(cfg.pad_ms),
            last_core_end: None,
            fed: 0,
            vad_origin: 0,
            history: VecDeque::with_capacity(history_cap),
            history_start: 0,
            history_cap,
            speech_start: None,
            cfg: cfg.clone(),
            pending: None,
        })
    }

    /// 要求換參數;會在下一次「沒在講話」時重建 VAD 套用,避免丟掉進行中的語音。
    pub fn set_params(&mut self, min_silence_sec: f32, max_speech_sec: f32) {
        if (self.cfg.min_silence_sec - min_silence_sec).abs() < 1e-3
            && (self.cfg.max_speech_sec - max_speech_sec).abs() < 1e-3
        {
            self.pending = None;
            return;
        }
        self.pending = Some((min_silence_sec, max_speech_sec));
    }

    /// 目前生效的(靜音秒數、單段上限秒數)。
    pub fn current_params(&self) -> (f32, f32) {
        (self.cfg.min_silence_sec, self.cfg.max_speech_sec)
    }

    fn apply_pending_if_idle(&mut self) {
        let Some((ms, mx)) = self.pending else { return };
        if self.vad.detected() || self.speech_start.is_some() {
            return;
        }
        let mut cfg = self.cfg.clone();
        cfg.min_silence_sec = ms;
        cfg.max_speech_sec = mx;
        match Self::build_vad(&cfg) {
            Ok(v) => {
                self.vad = v;
                self.vad_origin = self.fed;
                self.cfg = cfg;
                self.pending = None;
                tracing::debug!("VAD 參數已套用:靜音 {ms:.2}s、上限 {mx:.1}s");
            }
            Err(e) => {
                tracing::warn!("重建 VAD 失敗,維持舊參數:{e:#}");
                self.pending = None;
            }
        }
    }

    fn build_vad(cfg: &VadConfig) -> Result<VoiceActivityDetector> {
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
        VoiceActivityDetector::create(&config, 60.0)
            .ok_or_else(|| anyhow!("VoiceActivityDetector::create 失敗"))
    }

    /// 餵入樣本,回傳這次新切出的段落(可能 0 個或多個)。
    pub fn push(&mut self, samples: &[f32]) -> Vec<Segment> {
        self.apply_pending_if_idle();
        let before = self.fed;
        self.vad.accept_waveform(samples);
        self.fed += samples.len() as i64;
        self.history.extend(samples.iter().copied());
        while self.history.len() > self.history_cap {
            self.history.pop_front();
            self.history_start += 1;
        }
        let segs = self.drain(Instant::now());
        if !segs.is_empty() {
            // 段落剛結束:下一句從這之後算
            self.speech_start = None;
        }
        if self.speech_start.is_none() && self.vad.detected() {
            // 語音剛開始:往回多抓 pad,避免吃掉第一個音節;與正式段落同規則,不切進前一段
            self.speech_start = Some(
                (before - self.pre_pad)
                    .max(self.history_start)
                    .max(self.last_core_end.unwrap_or(i64::MIN)),
            );
        }
        segs
    }

    /// 目前講到一半的語音(從語音起點到最新樣本),供即時預覽辨識;沒在講話回 None。
    pub fn current_speech(&self) -> Option<Vec<f32>> {
        let start = self.speech_start?;
        if !self.vad.detected() {
            return None;
        }
        let a = (start - self.history_start).max(0) as usize;
        if a >= self.history.len() {
            return None;
        }
        Some(self.history.range(a..).copied().collect())
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
            let front = self
                .vad
                .front()
                .map(|seg| (self.vad_origin + seg.start() as i64, seg.n() as i64, seg.samples().to_vec()));
            self.vad.pop();
            if let Some((start, n, samples)) = front {
                out.push(self.with_padding(start, n, samples, now));
            }
        }
        out
    }

    fn with_padding(&mut self, start: i64, n: i64, core_samples: Vec<f32>, now: Instant) -> Segment {
        let max_n = self.cfg.max_speech_sec * TARGET_SAMPLE_RATE as f32;
        let forced = n as f32 >= max_n * 0.95;
        let hist = (self.history_start, self.history_start + self.history.len() as i64);
        let (want_start, want_end) =
            pad_bounds(start, start + n, self.pre_pad, self.post_pad, hist, self.last_core_end, forced);
        self.last_core_end = Some(start + n);
        let core = (start, start + n);
        // history 可能已不含這段(極端情況),就退回 VAD 給的原始樣本。
        if want_start >= want_end || start < self.history_start {
            return Segment {
                samples: core_samples,
                start_sample: start,
                end_at: now,
                core,
                forced,
            };
        }
        let a = (want_start - self.history_start) as usize;
        let b = (want_end - self.history_start) as usize;
        Segment {
            samples: self.history.range(a..b).copied().collect(),
            start_sample: want_start,
            end_at: now,
            core,
            forced,
        }
    }
}

/// 段落補白後的範圍 [start, end)(樣本索引)。
/// - 前段補 `pre`、後段補 `post`,都夾在 history 範圍 `hist` 內;
/// - 前段不得早於前一段語音本體的結尾 `prev_core_end`(避免邊界字在兩段各出現一次);
/// - 這段自己是強制切開的(`forced`):後面不補,下一段會從語音本體接著開始。
#[allow(clippy::too_many_arguments)]
pub fn pad_bounds(
    core_start: i64,
    core_end: i64,
    pre: i64,
    post: i64,
    hist: (i64, i64),
    prev_core_end: Option<i64>,
    forced: bool,
) -> (i64, i64) {
    let start = (core_start - pre)
        .max(prev_core_end.unwrap_or(i64::MIN))
        .max(hist.0);
    let end = (core_end + if forced { 0 } else { post }).min(hist.1);
    (start, end)
}

/// 斷句合併判斷(規格書):段落很短且無句末標點 → 等下一段合併。
///
/// 極短碎片(英文 ≤ 2 個詞、CJK ≤ 3 字)即使帶句號也視為碎片:
/// SenseVoice 的 ITN 會替「By.」「Mi.」補句號,單獨翻會得到「由於」「米」。
pub fn is_fragment(text: &str, lang_is_cjk: bool) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let body: String = t.chars().filter(|c| !c.is_ascii_punctuation() && !"。!?、,…「」".contains(*c)).collect();
    let tiny = if lang_is_cjk {
        body.chars().count() <= 3
    } else {
        body.split_whitespace().count() <= 2
    };
    if tiny {
        return true;
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
        assert!(is_fragment("ok.", false)); // 極短碎片即使有句號也合併
        assert!(is_fragment("By.", false));
        assert!(!is_fragment("See you all tomorrow.", false));
        assert!(is_fragment("それで", true));
        assert!(!is_fragment("それでは始めましょう", true));
        assert!(is_fragment("そう?", true)); // ≤ 3 字
        assert!(is_fragment("好在。", true)); // ≤ 3 字即使有句號也等待合併(逾時 1.5 s 後仍會送出)
        assert!(!is_fragment("", true));
    }

    // T17:沒有前段時前後都補白
    #[test]
    fn pad_bounds_no_prev_pads_both_sides() {
        assert_eq!(pad_bounds(16000, 32000, 8000, 3200, (0, 100000), None, false), (8000, 35200));
    }

    // T17:前一段本體結尾在補白範圍內時,起點不得早於它
    #[test]
    fn pad_bounds_start_not_before_prev_core_end() {
        assert_eq!(pad_bounds(16000, 32000, 8000, 3200, (0, 100000), Some(12000), false), (12000, 35200));
    }

    // T17:前一段語音本體結尾遠於 pre 範圍時,起點照補 preroll
    #[test]
    fn pad_bounds_far_prev_core_still_pre_pads() {
        assert_eq!(pad_bounds(16000, 32000, 8000, 3200, (0, 100000), Some(4000), false), (8000, 35200));
    }

    // T17:前一段語音本體結尾緊接本段起點時,起點不重疊
    #[test]
    fn pad_bounds_contiguous_prev_core_no_overlap() {
        assert_eq!(pad_bounds(16000, 32000, 8000, 3200, (0, 100000), Some(16000), false), (16000, 35200));
    }

    // T17:這段是強制切開時,後面不補白
    #[test]
    fn pad_bounds_forced_no_post_pad() {
        assert_eq!(pad_bounds(16000, 32000, 8000, 3200, (0, 100000), None, true), (8000, 32000));
    }

    // T17:補白超出 history 範圍時夾在範圍內
    #[test]
    fn pad_bounds_clamped_to_history() {
        assert_eq!(pad_bounds(16000, 32000, 8000, 3200, (10000, 33000), None, false), (10000, 33000));
    }
}
