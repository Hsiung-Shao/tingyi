//! 每句的時間戳與統計:`vad_end → asr_done → mt_first_token → mt_done`。

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
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

/// 管線執行期間的原子計數(worker 與 mt task 更新,前端每 2 s 讀)。
#[derive(Debug, Default)]
pub struct PipelineStats {
    pub segments: AtomicU64,
    pub asr_total_ms: AtomicU64,
    pub asr_last_ms: AtomicU64,
    pub preview_count: AtomicU64,
    pub preview_total_ms: AtomicU64,
    pub speaker_count: AtomicU64,
    pub speaker_total_ms: AtomicU64,
    pub speaker_last_ms: AtomicU64,
    pub mt_sent: AtomicU64,
    pub mt_done: AtomicU64,
    pub mt_first_total_ms: AtomicU64,
    pub mt_total_ms: AtomicU64,
    pub mt_last_first_ms: AtomicU64,
    pub merges: AtomicU64,
    /// 音訊 channel 目前積壓的 chunk 數(worker 每次迴圈更新)。
    pub audio_backlog: AtomicU64,
    pub audio_backlog_max: AtomicU64,
    pub speaking: AtomicBool,
    /// 最近 60 句端到端首字毫秒(環形)。
    e2e_recent: Mutex<VecDeque<u32>>,
}

impl PipelineStats {
    pub fn add(counter: &AtomicU64, v: u64) {
        counter.fetch_add(v, Ordering::Relaxed);
    }

    pub fn record_asr(&self, ms: u32) {
        self.segments.fetch_add(1, Ordering::Relaxed);
        self.asr_total_ms.fetch_add(ms as u64, Ordering::Relaxed);
        self.asr_last_ms.store(ms as u64, Ordering::Relaxed);
    }

    pub fn record_preview(&self, ms: u32) {
        self.preview_count.fetch_add(1, Ordering::Relaxed);
        self.preview_total_ms.fetch_add(ms as u64, Ordering::Relaxed);
    }

    pub fn record_speaker(&self, ms: u32) {
        self.speaker_count.fetch_add(1, Ordering::Relaxed);
        self.speaker_total_ms.fetch_add(ms as u64, Ordering::Relaxed);
        self.speaker_last_ms.store(ms as u64, Ordering::Relaxed);
    }

    pub fn record_mt(&self, first_ms: u32, total_ms: u32, e2e_first_ms: Option<u32>) {
        self.mt_done.fetch_add(1, Ordering::Relaxed);
        self.mt_first_total_ms.fetch_add(first_ms as u64, Ordering::Relaxed);
        self.mt_total_ms.fetch_add(total_ms as u64, Ordering::Relaxed);
        self.mt_last_first_ms.store(first_ms as u64, Ordering::Relaxed);
        if let Some(e) = e2e_first_ms {
            let mut q = self.e2e_recent.lock().unwrap();
            if q.len() >= 60 {
                q.pop_front();
            }
            q.push_back(e);
        }
    }

    pub fn set_backlog(&self, n: usize) {
        self.audio_backlog.store(n as u64, Ordering::Relaxed);
        self.audio_backlog_max.fetch_max(n as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let avg = |total: u64, n: u64| if n == 0 { 0 } else { (total / n) as u32 };
        let e2e_recent: Vec<u32> = self.e2e_recent.lock().unwrap().iter().copied().collect();
        StatsSnapshot {
            segments: g(&self.segments),
            asr_avg_ms: avg(g(&self.asr_total_ms), g(&self.segments)),
            asr_last_ms: g(&self.asr_last_ms) as u32,
            preview_count: g(&self.preview_count),
            preview_avg_ms: avg(g(&self.preview_total_ms), g(&self.preview_count)),
            speaker_count: g(&self.speaker_count),
            speaker_avg_ms: avg(g(&self.speaker_total_ms), g(&self.speaker_count)),
            speaker_last_ms: g(&self.speaker_last_ms) as u32,
            mt_queue: g(&self.mt_sent).saturating_sub(g(&self.mt_done)) as u32,
            mt_done: g(&self.mt_done),
            mt_first_avg_ms: avg(g(&self.mt_first_total_ms), g(&self.mt_done)),
            mt_total_avg_ms: avg(g(&self.mt_total_ms), g(&self.mt_done)),
            mt_last_first_ms: g(&self.mt_last_first_ms) as u32,
            merges: g(&self.merges),
            audio_backlog: g(&self.audio_backlog) as u32,
            audio_backlog_max: g(&self.audio_backlog_max) as u32,
            speaking: self.speaking.load(Ordering::Relaxed),
            e2e_p50: Stats::from_samples(e2e_recent.clone()).p50,
            e2e_recent,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct StatsSnapshot {
    pub segments: u64,
    pub asr_avg_ms: u32,
    pub asr_last_ms: u32,
    pub preview_count: u64,
    pub preview_avg_ms: u32,
    pub speaker_count: u64,
    pub speaker_avg_ms: u32,
    pub speaker_last_ms: u32,
    pub mt_queue: u32,
    pub mt_done: u64,
    pub mt_first_avg_ms: u32,
    pub mt_total_avg_ms: u32,
    pub mt_last_first_ms: u32,
    pub merges: u64,
    pub audio_backlog: u32,
    pub audio_backlog_max: u32,
    pub speaking: bool,
    pub e2e_p50: u32,
    pub e2e_recent: Vec<u32>,
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
    fn pipeline_stats_snapshot() {
        let s = PipelineStats::default();
        s.record_asr(100);
        s.record_asr(300);
        s.record_speaker(50);
        PipelineStats::add(&s.mt_sent, 2);
        s.record_mt(40, 120, Some(150));
        s.set_backlog(3);
        s.set_backlog(1);
        let snap = s.snapshot();
        assert_eq!(snap.segments, 2);
        assert_eq!(snap.asr_avg_ms, 200);
        assert_eq!(snap.asr_last_ms, 300);
        assert_eq!(snap.speaker_avg_ms, 50);
        assert_eq!(snap.mt_queue, 1);
        assert_eq!(snap.mt_first_avg_ms, 40);
        assert_eq!(snap.e2e_recent, vec![150]);
        assert_eq!(snap.e2e_p50, 150);
        assert_eq!(snap.audio_backlog, 1);
        assert_eq!(snap.audio_backlog_max, 3);
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
