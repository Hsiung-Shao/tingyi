//! 連續 16 kHz 樣本的環形緩衝,帶全域樣本時鐘。
//!
//! 與 `vad::Segmenter` 在同一個執行緒吃同一批 chunk,所以 `end()` 與 `Segmenter.fed`
//! 永遠相等,`Segment.start_sample` 可以直接拿來對這個緩衝取片段(回修用)。

use crate::audio::TARGET_SAMPLE_RATE;

pub struct AudioRing {
    buf: Vec<f32>,
    cap: usize,
    /// 下一個寫入位置(環內索引)。
    head: usize,
    /// 累計餵入的樣本數 = 下一個樣本的全域索引。
    total: i64,
}

impl AudioRing {
    pub fn with_seconds(secs: u32) -> Self {
        let cap = (secs as usize).max(1) * TARGET_SAMPLE_RATE as usize;
        Self::with_capacity(cap)
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: vec![0.0; cap.max(1)],
            cap: cap.max(1),
            head: 0,
            total: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.buf[self.head] = s;
            self.head = (self.head + 1) % self.cap;
        }
        self.total += samples.len() as i64;
    }

    /// 下一個樣本的全域索引(= 已餵入總數)。
    pub fn end(&self) -> i64 {
        self.total
    }

    /// 緩衝內最舊樣本的全域索引。
    pub fn start(&self) -> i64 {
        (self.total - self.cap as i64).max(0)
    }

    /// 取 `[a, b)` 的樣本;超出緩衝範圔或 a ≥ b 回 None。
    pub fn slice(&self, a: i64, b: i64) -> Option<Vec<f32>> {
        if a >= b || a < self.start() || b > self.end() {
            return None;
        }
        let n = (b - a) as usize;
        let mut out = Vec::with_capacity(n);
        // 全域索引 i 對應環內位置 i mod cap(因為 total 從 0 開始、head = total mod cap)
        let mut pos = (a % self.cap as i64) as usize;
        for _ in 0..n {
            out.push(self.buf[pos]);
            pos = (pos + 1) % self.cap;
        }
        Some(out)
    }
}

pub fn samples_to_ms(s: i64) -> u64 {
    (s.max(0) as u64) * 1000 / TARGET_SAMPLE_RATE as u64
}

pub fn ms_to_samples(ms: u64) -> i64 {
    (ms * TARGET_SAMPLE_RATE as u64 / 1000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_wraps_and_slices() {
        let mut r = AudioRing::with_capacity(10);
        let data: Vec<f32> = (0..25).map(|i| i as f32).collect();
        r.push(&data[..7]);
        r.push(&data[7..]);
        assert_eq!(r.end(), 25);
        assert_eq!(r.start(), 15);
        assert_eq!(r.slice(15, 20).unwrap(), vec![15.0, 16.0, 17.0, 18.0, 19.0]);
        assert_eq!(r.slice(23, 25).unwrap(), vec![23.0, 24.0]);
        assert!(r.slice(10, 12).is_none(), "已被覆蓋");
        assert!(r.slice(20, 26).is_none(), "超過尾端");
        assert!(r.slice(20, 20).is_none());
    }

    #[test]
    fn ms_conversions() {
        assert_eq!(samples_to_ms(16000), 1000);
        assert_eq!(ms_to_samples(1500), 24000);
        assert_eq!(samples_to_ms(-5), 0);
    }
}
