//! 懸浮字幕的閒置判斷:沒聲音(或沒人講話)且字幕沒有新內容超過設定秒數 → 通知字幕視窗收起。
//!
//! 事件轉發端把每個管線事件交給 `on_event`,並定時呼叫 `poll`;時間一律由呼叫端傳入,方便單元測試。
//! - 新內容:新句、有字的預覽、譯文串流/完成/失敗、合併。回修與補句不算(安靜時才會跑,不該讓舊句閃回)。
//! - 等譯文的句子未完成前不收起(超過 `PENDING_CAP` 不再擋,MT 卡住也能收)。
//! - 講話狀態卡在「講話中」但擷取封包停了(系統音訊沒有任何串流時)→ 以封包停的時長當靜音時長。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::pipeline::PipelineEvent;

/// 等譯文的句子最多擋這麼久(MT 逾時 15 s 加上合併等待)。
pub const PENDING_CAP: Duration = Duration::from_secs(20);
/// 使用者可設定的秒數範圍(0 = 不隱藏)。
pub const HIDE_SECS_RANGE: (f32, f32) = (1.0, 30.0);

/// 閒置隱藏設定;跨回合保留,執行中改了下一次 `poll` 就生效。
#[derive(Debug)]
pub struct IdleControl {
    /// 沒聲音多久收起字幕(毫秒);0 = 不隱藏。
    pub hide_ms: AtomicU32,
}

impl Default for IdleControl {
    fn default() -> Self {
        Self { hide_ms: AtomicU32::new(3000) }
    }
}

impl IdleControl {
    /// 套用使用者輸入的秒數:非有限值或 ≤ 0 → 0(不隱藏),其餘夾在 `HIDE_SECS_RANGE` 內。
    pub fn set_secs(&self, sec: f32) {
        let ms = if sec.is_finite() && sec > 0.0 {
            (sec.clamp(HIDE_SECS_RANGE.0, HIDE_SECS_RANGE.1) * 1000.0).round() as u32
        } else {
            0
        };
        self.hide_ms.store(ms, Ordering::Relaxed);
    }
}

/// 一個回合的閒置追蹤器(每次開始建立新的)。
#[derive(Debug)]
pub struct IdleTracker {
    translate: bool,
    last_content: Option<Instant>,
    /// 等譯文的句子 id → 到達時間。
    pending: HashMap<u64, Instant>,
    last_packets: u64,
    packets_at: Instant,
    fired: bool,
}

impl IdleTracker {
    /// `translate`:這個回合有沒有翻譯器;沒有就不會有譯文,句子不必等。
    pub fn new(translate: bool, now: Instant) -> Self {
        Self {
            translate,
            last_content: None,
            pending: HashMap::new(),
            last_packets: 0,
            packets_at: now,
            fired: false,
        }
    }

    pub fn on_event(&mut self, ev: &PipelineEvent, now: Instant) {
        match ev {
            PipelineEvent::Transcript { id, will_translate, .. } => {
                if self.translate && *will_translate {
                    self.pending.insert(*id, now);
                } else {
                    self.pending.remove(id);
                }
            }
            PipelineEvent::Partial { text, .. } if !text.is_empty() => {}
            PipelineEvent::TranslationDelta { .. } => {}
            PipelineEvent::TranslationDone { id, .. } | PipelineEvent::TranslationFailed { id, .. } => {
                self.pending.remove(id);
            }
            PipelineEvent::TranscriptMerged { from, .. } => {
                self.pending.remove(from);
            }
            _ => return,
        }
        self.last_content = Some(now);
        self.fired = false;
    }

    /// 回 true = 現在該收起字幕(同一段閒置只回一次,有新內容後重新計時)。
    /// `speaking` / `silence` 取自 `PipelineStats`,`packets` 為擷取封包累計數。
    pub fn poll(&mut self, now: Instant, hide_after: Duration, speaking: bool, silence: Duration, packets: u64) -> bool {
        if packets != self.last_packets {
            self.last_packets = packets;
            self.packets_at = now;
        }
        self.pending.retain(|_, t| now.saturating_duration_since(*t) < PENDING_CAP);
        let Some(content) = self.last_content else { return false };
        if self.fired || hide_after.is_zero() || !self.pending.is_empty() {
            return false;
        }
        // 講話中但封包停了:擷取沒有資料進來,講話狀態不會再更新,改看封包停了多久
        let quiet = if speaking { now.saturating_duration_since(self.packets_at) } else { silence };
        if quiet >= hide_after && now.saturating_duration_since(content) >= hide_after {
            self.fired = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::Lang;
    use crate::metrics::TimingSnapshot;
    use std::sync::atomic::Ordering;

    const H: Duration = Duration::from_secs(3);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn transcript(id: u64, will_translate: bool) -> PipelineEvent {
        PipelineEvent::Transcript {
            id,
            text: "hello".into(),
            lang: Lang::En,
            asr_ms: 10,
            will_translate,
            speaker: None,
            start_ms: 0,
            end_ms: 1000,
        }
    }

    fn done(id: u64) -> PipelineEvent {
        PipelineEvent::TranslationDone {
            id,
            source: "hello".into(),
            text: "你好".into(),
            timing: TimingSnapshot::default(),
            speaker: None,
        }
    }

    fn partial(text: &str) -> PipelineEvent {
        PipelineEvent::Partial { text: text.into(), lang: Lang::En, speaker: None }
    }

    /// 沒人講話、封包持續增加的 poll:靜音從 `silent_since_ms` 起算到 `at_ms`。
    fn quiet(tr: &mut IdleTracker, t0: Instant, at_ms: u64, silent_since_ms: u64, hide: Duration) -> bool {
        tr.poll(t0 + ms(at_ms), hide, false, ms(at_ms - silent_since_ms), at_ms + 1)
    }

    // T1:靜音且字幕逾時未更新,未滿 3 s 為 false、滿 3 s 為 true。
    #[test]
    fn silent_and_stale_notifies_at_threshold() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        assert!(!quiet(&mut tr, t0, 2900, 0, H), "未滿 3 s 不該收起");
        assert!(quiet(&mut tr, t0, 3000, 0, H), "滿 3 s 應收起");
    }

    // T2:講話中(speaking=true、silence=0、封包持續增加)即使內容很舊也不通知。
    #[test]
    fn speaking_with_flowing_packets_never_notifies() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        assert!(!tr.poll(t0 + ms(9500), H, true, ms(0), 1));
        assert!(!tr.poll(t0 + ms(10_000), H, true, ms(0), 2), "講話中不該收起");
    }

    // T3:譯文晚到時從譯文完成起算 3 s,而不是從原文起算。
    #[test]
    fn late_translation_restarts_the_clock() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(true, t0);
        tr.on_event(&transcript(1, true), t0);
        tr.on_event(&done(1), t0 + ms(5000));
        assert!(!quiet(&mut tr, t0, 7900, 0, H), "譯文後未滿 3 s 不該收起");
        assert!(quiet(&mut tr, t0, 8000, 0, H), "譯文後滿 3 s 應收起");
    }

    // T4:有待翻句未完成時,靜音再久也不通知;Done 後再滿 3 s 才通知。
    #[test]
    fn pending_translation_blocks_notification() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(true, t0);
        tr.on_event(&transcript(1, true), t0);
        assert!(!quiet(&mut tr, t0, 5000, 0, H), "等譯文時不該收起");
        tr.on_event(&done(1), t0 + ms(5000));
        assert!(quiet(&mut tr, t0, 8000, 0, H), "Done 後滿 3 s 應收起");
    }

    // T5:待翻句一直沒結果,到 PENDING_CAP 才不再擋。
    #[test]
    fn pending_translation_stops_blocking_after_cap() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(true, t0);
        tr.on_event(&transcript(1, true), t0);
        let cap = PENDING_CAP.as_millis() as u64;
        assert!(!quiet(&mut tr, t0, cap - 1000, 0, H), "上限前仍該被擋");
        assert!(quiet(&mut tr, t0, cap, 0, H), "到上限後不該再擋");
    }

    // T6:翻譯關閉時,will_translate=true 的句子不必等譯文,滿 3 s 即通知。
    #[test]
    fn no_translator_means_no_waiting() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, true), t0);
        assert!(quiet(&mut tr, t0, 3000, 0, H), "沒翻譯器不該等譯文");
    }

    // T7:被合併掉的句子(from)不再等譯文,只剩 into 的 Done 即可。
    #[test]
    fn merged_sentence_does_not_linger_as_pending() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(true, t0);
        tr.on_event(&transcript(1, true), t0);
        tr.on_event(&transcript(2, true), t0);
        tr.on_event(
            &PipelineEvent::TranscriptMerged { into: 1, from: 2, text: "hello hello".into(), start_ms: 0, end_ms: 2000 },
            t0 + ms(1000),
        );
        tr.on_event(&done(1), t0 + ms(2000));
        assert!(quiet(&mut tr, t0, 5000, 0, H), "id 2 不該殘留成待翻");
    }

    // T8:通知後只回一次;新句進來重新計時,滿 3 s 再通知一次。
    #[test]
    fn notifies_once_then_rearms_on_new_content() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        assert!(quiet(&mut tr, t0, 3000, 0, H));
        assert!(!quiet(&mut tr, t0, 4000, 0, H), "通知過不該重複");
        assert!(!quiet(&mut tr, t0, 10_000, 0, H), "通知過不該重複");
        tr.on_event(&transcript(2, false), t0 + ms(11_000));
        assert!(!quiet(&mut tr, t0, 13_900, 11_000, H), "新句後未滿 3 s 不該收起");
        assert!(quiet(&mut tr, t0, 14_000, 11_000, H), "新句後滿 3 s 應再次收起");
    }

    // T9:秒數 0 一律不隱藏;改成 3 s 的那次 poll 立即生效。
    #[test]
    fn zero_seconds_disables_and_change_applies_immediately() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        assert!(!quiet(&mut tr, t0, 30_000, 0, Duration::ZERO));
        assert!(!quiet(&mut tr, t0, 60_000, 0, Duration::ZERO), "hide_after=0 不該收起");
        assert!(quiet(&mut tr, t0, 61_000, 0, H), "改成 3 s 的那次就該收起");
    }

    // T10:SourceRevised / TranslationRevised / Recovered 不算新內容,不延後通知。
    #[test]
    fn revisions_do_not_postpone_notification() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(true, t0);
        tr.on_event(&transcript(1, false), t0);
        tr.on_event(&PipelineEvent::SourceRevised { id: 1, text: "hello!".into() }, t0 + ms(1000));
        tr.on_event(&PipelineEvent::TranslationRevised { id: 1, text: "你好!".into() }, t0 + ms(1100));
        tr.on_event(
            &PipelineEvent::Recovered {
                id: 9,
                after: Some(1),
                text: "extra".into(),
                lang: Lang::En,
                speaker: None,
                start_ms: 0,
                end_ms: 500,
                will_translate: false,
            },
            t0 + ms(1200),
        );
        assert!(quiet(&mut tr, t0, 3000, 0, H), "回修與補句不該延後收起");
    }

    // T11a:speaking 卡在 true 但封包停了 3 s,視為沒聲音而通知。
    #[test]
    fn stalled_packets_while_speaking_count_as_silence() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        assert!(!tr.poll(t0, H, true, ms(0), 5));
        assert!(!tr.poll(t0 + ms(2900), H, true, ms(0), 5), "封包停未滿 3 s 不該收起");
        assert!(tr.poll(t0 + ms(3000), H, true, ms(0), 5), "封包停滿 3 s 應收起");
    }

    // T11b:speaking=true 且封包持續增加時,即使內容很舊也不通知。
    #[test]
    fn flowing_packets_while_speaking_do_not_count_as_silence() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        for (i, at) in [0u64, 1000, 2000, 3000, 4000].iter().enumerate() {
            let r = tr.poll(t0 + ms(*at), H, true, ms(0), 5 + i as u64);
            assert!(!r, "封包持續增加不該收起(at={at})");
        }
    }

    // T12:只收到 Status / Segmentation 等非內容事件,從未顯示字幕,靜音再久也不通知。
    #[test]
    fn never_notifies_without_any_content() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(true, t0);
        tr.on_event(&PipelineEvent::Status { message: "ready".into() }, t0);
        tr.on_event(
            &PipelineEvent::Segmentation { min_silence_ms: 300, max_speech_ms: 8000, reason: "init".into() },
            t0,
        );
        tr.on_event(&PipelineEvent::Segment { id: 1, audio_ms: 500, rms_dbfs: -30.0, wav: None }, t0);
        tr.on_event(&PipelineEvent::Speakers { names: vec!["A".into()] }, t0);
        tr.on_event(&PipelineEvent::SpeakerMerged { from: "A".into(), into: "B".into() }, t0);
        tr.on_event(&PipelineEvent::Error { message: "x".into() }, t0);
        assert!(!quiet(&mut tr, t0, 60_000, 0, H), "從未有字幕內容不該通知");
    }

    // T13a:空字串預覽不算新內容,3 s 照常通知。
    #[test]
    fn empty_partial_is_not_new_content() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        tr.on_event(&partial(""), t0 + ms(2000));
        assert!(quiet(&mut tr, t0, 3000, 0, H), "空預覽不該延後收起");
    }

    // T13b:有字預覽算新內容,從預覽起算 3 s 才通知。
    #[test]
    fn non_empty_partial_counts_as_new_content() {
        let t0 = Instant::now();
        let mut tr = IdleTracker::new(false, t0);
        tr.on_event(&transcript(1, false), t0);
        tr.on_event(&partial("abc"), t0 + ms(2000));
        assert!(!quiet(&mut tr, t0, 3000, 0, H), "有字預覽後未滿 3 s 不該收起");
        assert!(quiet(&mut tr, t0, 5000, 0, H), "預覽後滿 3 s 應收起");
    }

    // T14:set_secs 對 0、負值、NaN 關閉隱藏,其餘夾在 1–30 s 換毫秒;Default 為 3000。
    #[test]
    fn set_secs_converts_and_clamps() {
        let c = IdleControl::default();
        assert_eq!(c.hide_ms.load(Ordering::Relaxed), 3000, "預設 3000 ms");
        let cases: [(f32, u32); 6] =
            [(0.0, 0), (-1.0, 0), (f32::NAN, 0), (0.5, 1000), (3.0, 3000), (100.0, 30_000)];
        for (sec, want) in cases {
            c.set_secs(sec);
            assert_eq!(c.hide_ms.load(Ordering::Relaxed), want, "set_secs({sec})");
        }
    }
}
