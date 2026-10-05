//! 每句的時間戳與統計:`vad_end → asr_done → mt_first_token → mt_done`。

use serde::Serialize;
use std::time::Instant;

/// 單句各階段時間戳(以 `Instant` 記錄,序列化時換成毫秒差)。
#[derive(Debug, Clone)]
pub struct SentenceTiming {
    pub id: u64,
    /// VAD 判定句尾(段落結束)的瞬間,所有延遲以此為原點。
    pub vad_end: Instant,
    pub asr_done: Option<Instant>,
    pub mt_first_token: Option<Instant>,
    pub mt_done: Option<Instant>,
    /// 段落音訊長度(毫秒)。
    pub audio_ms: u32,
}

impl SentenceTiming {
    pub fn new(id: u64, vad_end: Instant, audio_ms: u32) -> Self {
        Self {
            id,
            vad_end,
            asr_done: None,
            mt_first_token: None,
            mt_done: None,
            audio_ms,
        }
    }

    pub fn asr_ms(&self) -> Option<u32> {
        self.asr_done.map(|t| ms_between(self.vad_end, t))
    }

    /// 辨識完成 → 譯文第一個 token。
    pub fn mt_first_ms(&self) -> Option<u32> {
        match (self.asr_done, self.mt_first_token) {
            (Some(a), Some(b)) => Some(ms_between(a, b)),
            _ => None,
        }
    }

    pub fn mt_total_ms(&self) -> Option<u32> {
        match (self.asr_done, self.mt_done) {
            (Some(a), Some(b)) => Some(ms_between(a, b)),
            _ => None,
        }
    }

    /// 端到端:VAD 句尾 → 譯文第一個 token(規格書的「2 秒內出現譯文」)。
    pub fn e2e_first_ms(&self) -> Option<u32> {
        self.mt_first_token.map(|t| ms_between(self.vad_end, t))
    }

    pub fn e2e_done_ms(&self) -> Option<u32> {
        self.mt_done.map(|t| ms_between(self.vad_end, t))
    }

    pub fn snapshot(&self) -> TimingSnapshot {
        TimingSnapshot {
            id: self.id,
            audio_ms: self.audio_ms,
            asr_ms: self.asr_ms(),
            mt_first_ms: self.mt_first_ms(),
            mt_total_ms: self.mt_total_ms(),
            e2e_first_ms: self.e2e_first_ms(),
            e2e_done_ms: self.e2e_done_ms(),
        }
    }
}

fn ms_between(a: Instant, b: Instant) -> u32 {
    b.saturating_duration_since(a).as_millis().min(u32::MAX as u128) as u32
}

/// 可序列化的單句延遲快照(給前端與 bench JSONL)。
#[derive(Debug, Clone, Serialize, Default)]
pub struct TimingSnapshot {
    pub id: u64,
    pub audio_ms: u32,
    pub asr_ms: Option<u32>,
    pub mt_first_ms: Option<u32>,
    pub mt_total_ms: Option<u32>,
    pub e2e_first_ms: Option<u32>,
    pub e2e_done_ms: Option<u32>,
}

/// 一組樣本的 p50 / p95 / max / mean。
#[derive(Debug, Clone, Serialize, Default)]
pub struct Stats {
    pub n: usize,
    pub p50: u32,
    pub p95: u32,
    pub max: u32,
    pub mean: f32,
}

impl Stats {
    pub fn from_samples(mut v: Vec<u32>) -> Self {
        if v.is_empty() {
            return Self::default();
        }
        v.sort_unstable();
        let n = v.len();
        let pct = |p: f32| -> u32 {
            let idx = ((n as f32 - 1.0) * p).round() as usize;
            v[idx.min(n - 1)]
        };
        Self {
            n,
            p50: pct(0.50),
            p95: pct(0.95),
            max: v[n - 1],
            mean: v.iter().map(|&x| x as f32).sum::<f32>() / n as f32,
        }
    }
}

/// 彙整多句快照成各階段統計。
#[derive(Debug, Clone, Serialize, Default)]
pub struct Summary {
    pub asr: Stats,
    pub mt_first: Stats,
    pub mt_total: Stats,
    pub e2e_first: Stats,
    pub e2e_done: Stats,
}

impl Summary {
    pub fn from_snapshots(s: &[TimingSnapshot]) -> Self {
        let pick = |f: fn(&TimingSnapshot) -> Option<u32>| {
            Stats::from_samples(s.iter().filter_map(f).collect())
        };
        Self {
            asr: pick(|t| t.asr_ms),
            mt_first: pick(|t| t.mt_first_ms),
            mt_total: pick(|t| t.mt_total_ms),
            e2e_first: pick(|t| t.e2e_first_ms),
            e2e_done: pick(|t| t.e2e_done_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stats_percentiles() {
        let s = Stats::from_samples(vec![100, 200, 300, 400, 1000]);
        assert_eq!(s.n, 5);
        assert_eq!(s.p50, 300);
        assert_eq!(s.p95, 1000);
        assert_eq!(s.max, 1000);
        assert!((s.mean - 400.0).abs() < 0.01);
        assert_eq!(Stats::from_samples(vec![]).n, 0);
    }

    #[test]
    fn timing_deltas() {
        let t0 = Instant::now();
        let mut t = SentenceTiming::new(1, t0, 3000);
        t.asr_done = Some(t0 + Duration::from_millis(200));
        t.mt_first_token = Some(t0 + Duration::from_millis(450));
        t.mt_done = Some(t0 + Duration::from_millis(900));
        assert_eq!(t.asr_ms(), Some(200));
        assert_eq!(t.mt_first_ms(), Some(250));
        assert_eq!(t.mt_total_ms(), Some(700));
        assert_eq!(t.e2e_first_ms(), Some(450));
        assert_eq!(t.e2e_done_ms(), Some(900));
    }
}
