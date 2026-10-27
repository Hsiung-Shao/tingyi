//! Silero VAD 切段(透過 sherpa-onnx)。
//!
//! sherpa 只切「小段」(最短靜音 0.1 s),何時把小段合併送出由 `endpoint::Endpointer` 決定
//! (短組要停頓 `min_silence_sec`、組長達 `max_speech_sec` 硬切);段首段尾再補緩衝。

use anyhow::{anyhow, Result};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use crate::audio::TARGET_SAMPLE_RATE;
use crate::endpoint::{quietest_frame, Emit, EndpointParams, Endpointer};

/// 沒有指定、也沒有引擎預設時的段首緩衝(毫秒)。
pub const DEFAULT_PRE_PAD_MS: u32 = 200;

/// sherpa VAD 的最短靜音:只用來切小段,送出時機交給 Endpointer。
const SMALL_SILENCE_SEC: f32 = 0.1;
/// sherpa VAD 的語音上限:只是防呆,實際上限由 Endpointer 硬切。
const SHERPA_MAX_SPEECH_SEC: f32 = 20.0;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub model: PathBuf,
    pub threshold: f32,
    /// 短組需要的停頓秒數(Endpointer 的 `base`;調整器的靜音門檻)。
    pub min_silence_sec: f32,
    pub min_speech_sec: f32,
    /// 單段硬上限秒數(Endpointer 的 `max_len`)。
    pub max_speech_sec: f32,
    /// 組長達到這個秒數後,小段一結束就送出(Endpointer 的 `long_len`)。
    pub long_len_sec: f32,
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
            // 短組停頓 0.35 s、硬上限 5 s(2026-10-06 真實影片實測後自規格書 0.5 s / 8 s 調低);
            // 執行中由調整器透過 `Segmenter::set_params` 改寫。
            threshold: 0.5,
            min_silence_sec: 0.35,
            // 0.25 → 0.128 s:起音盲區(Endpointer 的 onset_lag)跟著縮短,短句送出快約 0.13 s(2026-10-27 真實日文素材)
            min_speech_sec: 0.128,
            max_speech_sec: 5.0,
            long_len_sec: 2.5,
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
    /// 段落被送出的瞬間(端到端延遲原點)。
    pub end_at: Instant,
    /// 語音本體 [start, end)(不含補白);斷句調整、說話者、字速都用這段。
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
    endpointer: Endpointer,
    pre_pad: i64,
    post_pad: i64,
    /// 上一段語音本體的結尾(補白不切進前一段)。
    last_core_end: Option<i64>,
    /// 已餵進 Segmenter 的總樣本數(sherpa 小段的 start 也是從 0 起算的絕對位置)。
    fed: i64,
    /// 最近的原始樣本,供補前後緩衝用。
    history: VecDeque<f32>,
    history_start: i64,
    history_cap: usize,
    /// VAD 回報開口時,往回推估實際開口的樣本數(min_speech + 2 窗)。
    onset_back: i64,
    /// VAD 正偵測到語音時,這段語音的估計起點。
    onset: Option<i64>,
}

impl Segmenter {
    pub fn new(cfg: &VadConfig) -> Result<Self> {
        let vad = Self::build_vad(cfg)?;
        let ms_to_samples = |ms: u32| ms as i64 * TARGET_SAMPLE_RATE as i64 / 1000;
        let sec_to_ms = |s: f32| (s * 1000.0).round() as u32;
        // history 要能容納使用者可填的最大單段上限 + 緩衝:合併後的段落只能從 history 取樣本
        let max_sec = cfg.max_speech_sec.max(crate::tuner::MAX_SPEECH_INPUT_MS.1 as f32 / 1000.0);
        let history_cap = (max_sec.ceil() as usize + 4) * TARGET_SAMPLE_RATE as usize;
        Ok(Self {
            vad,
            endpointer: Endpointer::new(EndpointParams {
                long_len: ms_to_samples(sec_to_ms(cfg.long_len_sec)),
                ..EndpointParams::new(sec_to_ms(cfg.min_silence_sec), sec_to_ms(cfg.max_speech_sec), sec_to_ms(cfg.min_speech_sec))
            }),
            pre_pad: ms_to_samples(cfg.pre_pad_ms.unwrap_or(DEFAULT_PRE_PAD_MS)),
            post_pad: ms_to_samples(cfg.pad_ms),
            last_core_end: None,
            fed: 0,
            history: VecDeque::with_capacity(history_cap),
            history_start: 0,
            history_cap,
            onset_back: (cfg.min_speech_sec * TARGET_SAMPLE_RATE as f32).round() as i64 + 2 * cfg.window_size as i64,
            onset: None,
        })
    }

    /// 換參數(短組需要的停頓秒數、單段硬上限秒數);下一次餵樣本就生效。
    pub fn set_params(&mut self, min_silence_sec: f32, max_speech_sec: f32) {
        let to_samples = |s: f32| (s * TARGET_SAMPLE_RATE as f32).round() as i64;
        self.endpointer.set_params(to_samples(min_silence_sec), to_samples(max_speech_sec));
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
                min_silence_duration: SMALL_SILENCE_SEC,
                min_speech_duration: cfg.min_speech_sec,
                window_size: cfg.window_size,
                max_speech_duration: SHERPA_MAX_SPEECH_SEC,
            },
            sample_rate: TARGET_SAMPLE_RATE,
            num_threads: cfg.num_threads,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        };
        // VAD 內部緩衝 60 秒,足以容納 sherpa 上限 + 靜音。
        VoiceActivityDetector::create(&config, 60.0)
            .ok_or_else(|| anyhow!("VoiceActivityDetector::create 失敗"))
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
        if self.vad.detected() {
            self.onset.get_or_insert(self.fed - self.onset_back);
        } else {
            self.onset = None;
        }
        let mut emits = self.deliver_smalls();
        let (history, hs) = (&self.history, self.history_start);
        emits.extend(self.endpointer.tick(self.fed, self.onset, &mut |lo, hi| {
            let lo = lo.max(hs);
            let x: Vec<f32> = history.range((lo - hs) as usize..(hi - hs).max(lo - hs) as usize).copied().collect();
            // 100 ms 框比 20 ms 不易落在字中間(2026-10-27 串流 CER:6 組中 5 組改善 0.2–1.8 個百分點)
            quietest_frame(&x, lo, 1600)
        }));
        self.emit_segments(emits)
    }

    /// 目前講到一半的語音(從開著的組首到最新樣本),供即時預覽辨識;沒有組回 None。
    /// VAD 偵測到語音時 `tick` 一定已開組,所以不必另看 onset。
    pub fn current_speech(&self) -> Option<Vec<f32>> {
        let start = (self.endpointer.open_start()? - self.pre_pad)
            .max(self.last_core_end.unwrap_or(i64::MIN))
            .max(self.history_start);
        let a = (start - self.history_start) as usize;
        if a >= self.history.len() {
            return None;
        }
        Some(self.history.range(a..).copied().collect())
    }

    /// 講者是否還在講(即時預覽、翻譯與回修的靜音計時用)。
    pub fn speaking(&self) -> bool {
        self.endpointer.speaking(self.fed, self.vad.detected())
    }

    /// 強制結束目前段落(停止擷取時呼叫)。
    pub fn flush(&mut self) -> Vec<Segment> {
        self.vad.flush();
        let mut emits = self.deliver_smalls();
        emits.extend(self.endpointer.finish_group(self.fed));
        self.emit_segments(emits)
    }

    /// 結束目前的組但不 flush VAD(切換音訊來源時,插完靜音後呼叫)。
    pub fn finish_group(&mut self) -> Vec<Segment> {
        let emits = self.endpointer.finish_group(self.fed).into_iter().collect();
        self.emit_segments(emits)
    }

    /// 把 VAD 切好的小段依序交給 Endpointer。
    fn deliver_smalls(&mut self) -> Vec<Emit> {
        let mut out = Vec::new();
        while !self.vad.is_empty() {
            let core = self.vad.front().map(|s| (s.start() as i64, s.start() as i64 + s.n() as i64));
            self.vad.pop();
            if let Some(core) = core {
                out.extend(self.endpointer.on_small(core, self.fed));
            }
        }
        out
    }

    fn emit_segments(&mut self, emits: Vec<Emit>) -> Vec<Segment> {
        let now = Instant::now();
        emits.into_iter().filter_map(|e| self.with_padding(e, now)).collect()
    }

    fn with_padding(&mut self, e: Emit, now: Instant) -> Option<Segment> {
        let hist = (self.history_start, self.history_start + self.history.len() as i64);
        let (want_start, want_end) =
            pad_bounds(e.core.0, e.core.1, self.pre_pad, self.post_pad.min(e.post_limit), hist, self.last_core_end, e.forced);
        self.last_core_end = Some(e.core.1);
        // pad_bounds 已夾在 history 內,history 不含整段時只交出還留著的部分;
        // 整段都已被擠出 history(極端情況)就沒有樣本可送,丟掉。
        if want_start >= want_end {
            tracing::warn!("段落 {:?} 已不在 history 內,丟棄", e.core);
            return None;
        }
        let a = (want_start - self.history_start) as usize;
        let b = (want_end - self.history_start) as usize;
        Some(Segment {
            samples: self.history.range(a..b).copied().collect(),
            start_sample: want_start,
            end_at: now,
            core: e.core,
            forced: e.forced,
        })
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
