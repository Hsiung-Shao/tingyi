//! 斷句自適應:依語言給預設靜音門檻,再依最近段落的語速與切段結果微調,
//! 疊上使用者的快/中/慢偏移。輸出給 `Segmenter::set_params` 熱換 VAD 參數。

use crate::asr::Lang;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

/// 跨執行緒的控制面:Tauri 命令改原子值,worker 下一段就套用。
#[derive(Debug)]
pub struct SegControl {
    /// 使用者偏移(毫秒):快 −100 / 中 0 / 慢 +100。
    pub offset_ms: AtomicI32,
    /// 是否自動調整(關掉 = 只用語言預設 + 偏移)。
    pub adaptive: AtomicBool,
    /// 目前生效的靜音門檻(毫秒),供 UI 顯示。
    pub current_ms: AtomicU32,
    pub current_max_speech_ms: AtomicU32,
}

impl Default for SegControl {
    fn default() -> Self {
        Self {
            offset_ms: AtomicI32::new(0),
            adaptive: AtomicBool::new(true),
            current_ms: AtomicU32::new(350),
            current_max_speech_ms: AtomicU32::new(5000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegParams {
    pub min_silence_ms: u32,
    pub max_speech_ms: u32,
    pub reason: String,
}

/// 一段的觀察值。
#[derive(Debug, Clone, Copy)]
pub struct SegStat {
    pub audio_ms: u32,
    pub chars: usize,
    /// 撞到單段上限才切
    pub hit_max: bool,
    /// 文字被判為「沒說完」(會等下一段合併)
    pub incomplete: bool,
}

pub struct SegmentationTuner {
    lang: Option<Lang>,
    /// 自適應累積的調整量(毫秒,可負)。
    adapt_ms: i32,
    history: VecDeque<SegStat>,
    cooldown: u32,
    last: Option<SegParams>,
}

const MIN_MS: i32 = 200;
const MAX_MS: i32 = 600;
const WINDOW: usize = 8;
const COOLDOWN_SEGMENTS: u32 = 4;
const STEP_MS: i32 = 50;

impl Default for SegmentationTuner {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentationTuner {
    pub fn new() -> Self {
        Self {
            lang: None,
            adapt_ms: 0,
            history: VecDeque::with_capacity(WINDOW),
            cooldown: 0,
            last: None,
        }
    }

    /// 依語言的預設靜音門檻(毫秒)。日文語速快、停頓短;英文句間停頓較長。
    pub fn lang_default_ms(lang: Option<Lang>) -> u32 {
        match lang {
            Some(Lang::Ja) => 300,
            Some(Lang::Ko) => 350,
            Some(Lang::En) => 450,
            Some(Lang::Zh) => 350,
            _ => 350,
        }
    }

    /// 語言改變時重置自適應量;回傳新參數(若有變)。
    pub fn set_lang(&mut self, lang: Lang, control: &SegControl) -> Option<SegParams> {
        if lang == Lang::Unknown || self.lang == Some(lang) {
            return None;
        }
        self.lang = Some(lang);
        self.adapt_ms = 0;
        self.history.clear();
        self.cooldown = 0;
        self.emit(control, format!("語言 {} 預設", lang.code()))
    }

    /// 餵入一段的觀察值;回傳新參數(若有變)。
    pub fn observe(&mut self, stat: SegStat, control: &SegControl) -> Option<SegParams> {
        if self.history.len() == WINDOW {
            self.history.pop_front();
        }
        self.history.push_back(stat);
        if !control.adaptive.load(Ordering::Relaxed) {
            return self.emit(control, "手動".into());
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return self.emit_if_changed(control);
        }
        if self.history.len() < 4 {
            return self.emit_if_changed(control);
        }
        let n = self.history.len() as f32;
        let hit_rate = self.history.iter().filter(|s| s.hit_max).count() as f32 / n;
        let frag_rate = self
            .history
            .iter()
            .filter(|s| s.audio_ms < 1200 && s.incomplete)
            .count() as f32
            / n;
        let total_ms: u32 = self.history.iter().map(|s| s.audio_ms).sum();
        let total_chars: usize = self.history.iter().map(|s| s.chars).sum();
        let cps = if total_ms > 0 { total_chars as f32 * 1000.0 / total_ms as f32 } else { 0.0 };
        let fast_threshold = match self.lang {
            Some(Lang::En) => 3.5, // 詞/s(chars 給的是詞數)
            _ => 6.0,              // 字/s
        };

        let mut reason = None;
        if hit_rate >= 0.5 || cps >= fast_threshold {
            if self.adapt_ms > MIN_MS - Self::lang_default_ms(self.lang) as i32 {
                self.adapt_ms -= STEP_MS;
                reason = Some(if hit_rate >= 0.5 { "常撞單段上限,縮短靜音門檻" } else { "語速快,縮短靜音門檻" });
            }
        } else if frag_rate >= 0.5 {
            if self.adapt_ms < MAX_MS - Self::lang_default_ms(self.lang) as i32 {
                self.adapt_ms += STEP_MS;
                reason = Some("碎片多,放寬靜音門檻");
            }
        }
        if let Some(r) = reason {
            self.cooldown = COOLDOWN_SEGMENTS;
            self.history.clear();
            return self.emit(control, r.into());
        }
        self.emit_if_changed(control)
    }

    /// 目前參數(語言預設 + 自適應 + 使用者偏移,夾在 200–600 ms)。
    pub fn params(&self, control: &SegControl) -> SegParams {
        let base = Self::lang_default_ms(self.lang) as i32;
        let adapt = if control.adaptive.load(Ordering::Relaxed) { self.adapt_ms } else { 0 };
        let offset = control.offset_ms.load(Ordering::Relaxed);
        let ms = (base + adapt + offset).clamp(MIN_MS, MAX_MS) as u32;
        // 語速快(門檻低)→ 單段上限 4 s;慢 → 6 s;否則 5 s
        let max_speech_ms = if ms <= 300 { 4000 } else if ms >= 500 { 6000 } else { 5000 };
        SegParams {
            min_silence_ms: ms,
            max_speech_ms,
            reason: String::new(),
        }
    }

    fn emit(&mut self, control: &SegControl, reason: String) -> Option<SegParams> {
        let mut p = self.params(control);
        p.reason = reason;
        control.current_ms.store(p.min_silence_ms, Ordering::Relaxed);
        control
            .current_max_speech_ms
            .store(p.max_speech_ms, Ordering::Relaxed);
        self.last = Some(p.clone());
        Some(p)
    }

    /// 偏移或自動開關被使用者改了 → 參數變了才回傳。
    pub fn emit_if_changed(&mut self, control: &SegControl) -> Option<SegParams> {
        let p = self.params(control);
        let changed = match &self.last {
            Some(l) => l.min_silence_ms != p.min_silence_ms || l.max_speech_ms != p.max_speech_ms,
            None => true,
        };
        if changed {
            self.emit(control, "使用者調整".into())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(audio_ms: u32, chars: usize, hit_max: bool, incomplete: bool) -> SegStat {
        SegStat { audio_ms, chars, hit_max, incomplete }
    }

    #[test]
    fn language_defaults_and_offset() {
        let c = SegControl::default();
        let mut t = SegmentationTuner::new();
        let p = t.set_lang(Lang::Ja, &c).unwrap();
        assert_eq!(p.min_silence_ms, 300);
        assert_eq!(p.max_speech_ms, 4000);
        c.offset_ms.store(100, Ordering::Relaxed);
        let p = t.emit_if_changed(&c).unwrap();
        assert_eq!(p.min_silence_ms, 400);
        assert!(t.emit_if_changed(&c).is_none(), "沒變就不重送");
        assert_eq!(c.current_ms.load(Ordering::Relaxed), 400);
    }

    #[test]
    fn hitting_max_lowers_threshold_with_cooldown() {
        let c = SegControl::default();
        let mut t = SegmentationTuner::new();
        t.set_lang(Lang::En, &c);
        let mut last = None;
        for _ in 0..4 {
            last = t.observe(stat(5000, 12, true, false), &c).or(last);
        }
        let p = last.expect("第 4 段應調整");
        assert_eq!(p.min_silence_ms, 400);
        assert!(p.reason.contains("撞單段上限"));
        // 冷卻期間不再調
        for _ in 0..4 {
            assert!(t.observe(stat(5000, 12, true, false), &c).is_none());
        }
        // 冷卻結束再累積 4 段後才會再降
        let mut again = None;
        for _ in 0..4 {
            again = t.observe(stat(5000, 12, true, false), &c).or(again);
        }
        assert_eq!(again.unwrap().min_silence_ms, 350);
    }

    #[test]
    fn fragments_raise_threshold_and_clamp() {
        let c = SegControl::default();
        let mut t = SegmentationTuner::new();
        t.set_lang(Lang::Ja, &c);
        let mut last = None;
        for _ in 0..4 {
            last = t.observe(stat(800, 3, false, true), &c).or(last);
        }
        assert_eq!(last.unwrap().min_silence_ms, 350);
        // 關掉自動 → 回到語言預設 + 偏移
        c.adaptive.store(false, Ordering::Relaxed);
        let p = t.observe(stat(800, 3, false, true), &c).unwrap();
        assert_eq!(p.min_silence_ms, 300);
        assert_eq!(p.reason, "手動");
        // 夾限:偏移 +100 且不自動,仍 ≤ 600
        c.offset_ms.store(500, Ordering::Relaxed);
        assert_eq!(t.params(&c).min_silence_ms, 600);
    }

    #[test]
    fn stable_speech_does_not_change() {
        let c = SegControl::default();
        let mut t = SegmentationTuner::new();
        t.set_lang(Lang::Ja, &c);
        for _ in 0..8 {
            assert!(t.observe(stat(2500, 10, false, false), &c).is_none());
        }
        assert_eq!(t.params(&c).min_silence_ms, 300);
    }
}
