//! 斷句合併器:把 VAD 切出的「小段」(最短靜音 0.1 s)合併成送辨識的段落。
//!
//! sherpa 的 `max_speech_duration` 只是軟上限,連續語音會長到 8–31 s;句間停頓又常被底噪蓋住。
//! 所以 VAD 只負責找小段,何時送出由這裡決定:
//! - 組長 < `long_len`:要停頓 `base`(調整器的門檻)才送出;因為 VAD 要連續語音 min_speech 後才回報開口,
//!   沒看到開口時要等 `base + onset_lag` 才能確定這段停頓裡沒人開口。
//! - 組長 ≥ `long_len`:小段一結束就送出。
//! - 組長達 `max_len`:在最近 `cut_search` 內(距組首至少 `cut_min_head`)找最安靜的框硬切。
//!
//! 單位一律是 16 kHz 樣本索引(整條串流的絕對位置),不碰 sherpa 也不碰時間,方便單元測試。

/// 合併器參數(樣本數)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointParams {
    /// 短組需要的停頓(調整器的靜音門檻)。
    pub base: i64,
    /// 硬上限:組長(含進行中語音)達到就硬切。
    pub max_len: i64,
    /// VAD 的最短靜音(小段之間至少隔這麼久)。
    pub small_gap: i64,
    /// 起音盲區:實際開口到 VAD 回報開口的最長延遲(min_speech + 2 窗 + 1 塊)。
    pub onset_lag: i64,
    /// 組長達到這個長度後,小段一結束就送出。
    pub long_len: i64,
    /// 單一小段組成的組,語音總長小於這個值就丟掉(雜訊)。
    pub min_keep: i64,
    /// 硬切時往回找切點的範圍。
    pub cut_search: i64,
    /// 硬切點距組首至少這麼遠。
    pub cut_min_head: i64,
    /// 後補白上限。
    pub max_post: i64,
}

impl EndpointParams {
    /// 以毫秒指定門檻、上限與 VAD 的 min_speech,其餘用預設值。
    pub fn new(base_ms: u32, max_ms: u32, min_speech_ms: u32) -> Self {
        let ms = |v: u32| v as i64 * 16;
        Self {
            base: ms(base_ms),
            max_len: ms(max_ms),
            small_gap: ms(100),
            // min_speech + 2 個 Silero 窗(512)+ 1 塊擷取(480)
            onset_lag: ms(min_speech_ms) + 2 * 512 + 480,
            long_len: ms(2500),
            min_keep: ms(250),
            cut_search: ms(1500),
            cut_min_head: ms(1000),
            max_post: ms(200),
        }
    }
}

/// 一個要送出的段落(語音本體範圍)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emit {
    /// 語音本體 [start, end)。
    pub core: (i64, i64),
    /// 撞硬上限被切開(話還沒講完)。
    pub forced: bool,
    /// 後補白最多幾個樣本(已確認是非語音的範圍);forced 為 0。
    pub post_limit: i64,
}

/// 組長 `group_len` 時,需要多長的停頓才送出:< `long_len` 為 `base`,否則 0。
pub fn need_gap(group_len: i64, p: &EndpointParams) -> i64 {
    if group_len < p.long_len {
        p.base
    } else {
        0
    }
}

/// `x` 是從 `origin` 開始的樣本;以 `frame` 個樣本為一框(從頭不重疊切),回傳 RMS 最小那框的中點
/// (絕對樣本索引)。同為最小取最後一框;不足一框回傳 `origin + x.len() / 2`。
pub fn quietest_frame(x: &[f32], origin: i64, frame: usize) -> i64 {
    let mut best: Option<(usize, f32)> = None;
    for (i, f) in x.chunks_exact(frame).enumerate() {
        let energy: f32 = f.iter().map(|v| v * v).sum();
        if best.is_none_or(|(_, e)| energy <= e) {
            best = Some((i, energy));
        }
    }
    match best {
        Some((i, _)) => origin + (i * frame + frame / 2) as i64,
        None => origin + x.len() as i64 / 2,
    }
}

/// 開著的組。
struct Group {
    start: i64,
    /// 組尾(最後一個小段的結尾);None = 還沒有小段交付(由開口或硬切開的組)。
    end: Option<i64>,
    smalls: u32,
    /// 小段語音總長。
    speech: i64,
    /// 硬切後接續的組:不適用 `min_keep` 丟棄。
    after_cut: bool,
}

/// 斷句合併器。
pub struct Endpointer {
    p: EndpointParams,
    group: Option<Group>,
    /// 上次硬切點;之後交付的小段要裁到這之後。
    cut: i64,
    /// 最後一個小段的結尾(組送出後仍保留,`speaking` 用)。
    last_end: Option<i64>,
}

impl Endpointer {
    pub fn new(p: EndpointParams) -> Self {
        Self { p, group: None, cut: i64::MIN, last_end: None }
    }

    pub fn params(&self) -> &EndpointParams {
        &self.p
    }

    /// 調整器換參數:立即生效(下一次 `tick` 就用新值)。
    pub fn set_params(&mut self, base: i64, max_len: i64) {
        self.p.base = base;
        self.p.max_len = max_len;
    }

    /// VAD 交付一個小段 `core = (a, b)`;`fed` 是交付當下已餵入的樣本數。
    /// - 先依上次硬切點裁切:`b ≤ 切點` 整段丟掉;`a < 切點` 裁成 `(切點, b)`。
    /// - 有開著的組且停頓 `a − 組尾 ≥ need_gap(組長)` → 先送出舊組(回傳),再以這個小段開新組。
    /// - 否則併入開著的組;沒有組就開新組。
    pub fn on_small(&mut self, core: (i64, i64), fed: i64) -> Option<Emit> {
        let (a, b) = (core.0.max(self.cut), core.1);
        if b <= self.cut {
            return None;
        }
        self.last_end = Some(b);
        let split = self
            .group
            .as_ref()
            .and_then(|g| g.end.map(|end| a - end >= need_gap(end - g.start, &self.p)))
            .unwrap_or(false);
        let out = if split { self.take(fed) } else { None };
        match &mut self.group {
            Some(g) => {
                g.start = g.start.min(a);
                g.end = Some(b);
                g.smalls += 1;
                g.speech += b - a;
            }
            None => self.group = Some(Group { start: a, end: Some(b), smalls: 1, speech: b - a, after_cut: false }),
        }
        out
    }

    /// 每次餵完樣本呼叫。`onset`:VAD 正偵測到語音時為進行中語音的估計起點,否則 None。
    /// `quietest(lo, hi)` 回傳 [lo, hi) 內最安靜的位置(硬切用)。依序判斷:
    /// - 組長 ≥ `long_len` 且組尾已知 → 立即送出。
    /// - 短組:`onset` 晚於組尾且 `onset − 組尾 ≥ base` → 立即送出;沒有 `onset` 且 `fed − 組尾 ≥ base + onset_lag` → 送出。
    /// - 組尾未知且沒有 `onset` → 清掉這個組(小段已在 tick 前交付完,不會再有屬於它的語音)。
    /// - 組長(有 `onset` 時算到 `fed`,否則算到組尾;記為 reach)≥ `max_len` → 在
    ///   [max(reach − cut_search, 組首 + cut_min_head), reach) 硬切,前半 forced 送出,後半從切點開新組。
    ///
    /// 非 forced 的 `post_limit = min(max_post, max(small_gap, fed − 組尾 − onset_lag))`。
    /// 單一小段的組(不是硬切後接續的)語音總長 < `min_keep` 時丟掉不送。
    pub fn tick(&mut self, fed: i64, onset: Option<i64>, quietest: &mut dyn FnMut(i64, i64) -> i64) -> Vec<Emit> {
        let mut out = Vec::new();
        let due = self.group.as_ref().and_then(|g| {
            let end = g.end?;
            let gap = need_gap(end - g.start, &self.p);
            Some(
                gap == 0
                    || match onset {
                        Some(o) => o - end >= gap,
                        None => fed - end >= gap + self.p.onset_lag,
                    },
            )
        });
        if due == Some(true) {
            out.extend(self.take(fed));
        }
        if onset.is_none() && self.group.as_ref().is_some_and(|g| g.end.is_none()) {
            self.group = None;
        }
        if let (None, Some(o)) = (&self.group, onset) {
            self.group = Some(Group { start: o.max(self.cut), end: None, smalls: 0, speech: 0, after_cut: false });
        }
        if let Some(g) = &mut self.group {
            let reach = if onset.is_some() { fed } else { g.end.unwrap_or(g.start) };
            if reach - g.start >= self.p.max_len {
                let c = quietest((reach - self.p.cut_search).max(g.start + self.p.cut_min_head), reach);
                out.push(Emit { core: (g.start, c), forced: true, post_limit: 0 });
                // 沒有 onset 時是已交付的小段剛好跨過上限:後半的組尾已知,照常等停頓送出
                let end = g.end.filter(|&e| e > c);
                *g = Group { start: c, end, smalls: 0, speech: end.map_or(0, |e| e - c), after_cut: true };
                self.cut = c;
            }
        }
        out
    }

    /// 停止或切換音訊來源:送出開著的組(組尾未知回 None),並清空狀態。
    pub fn finish_group(&mut self, fed: i64) -> Option<Emit> {
        self.take(fed)
    }

    /// 開著的組的起點(預覽用);沒有組回 None。
    pub fn open_start(&self) -> Option<i64> {
        self.group.as_ref().map(|g| g.start)
    }

    /// 講者是否還在講(翻譯與回修的靜音計時用):VAD 偵測到、有組開著,或距最後一個小段結尾 < `onset_lag`。
    pub fn speaking(&self, fed: i64, detected: bool) -> bool {
        detected || self.group.is_some() || self.last_end.is_some_and(|e| fed - e < self.p.onset_lag)
    }

    /// 關掉開著的組並轉成 Emit;組尾未知或是孤立雜訊回 None。
    fn take(&mut self, fed: i64) -> Option<Emit> {
        let g = self.group.take()?;
        let end = g.end?;
        if g.smalls == 1 && !g.after_cut && g.speech < self.p.min_keep {
            return None;
        }
        let post_limit = self.p.max_post.min(self.p.small_gap.max(fed - end - self.p.onset_lag));
        Some(Emit { core: (g.start, end), forced: false, post_limit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: i64 = 480;

    fn p() -> EndpointParams {
        EndpointParams::new(300, 4000, 250)
    }

    /// 最安靜位置固定取 hi - 1600(離 fed 100 ms)。
    fn tick_q(e: &mut Endpointer, fed: i64, onset: Option<i64>) -> Vec<Emit> {
        e.tick(fed, onset, &mut |_, hi| hi - 1600)
    }

    /// 每塊 tick 一次,回傳所有送出與第一次送出的 fed。
    fn run_ticks(e: &mut Endpointer, from: i64, to: i64, onset: Option<i64>) -> (Vec<Emit>, Option<i64>) {
        let mut all = Vec::new();
        let mut first = None;
        let mut fed = from;
        while fed <= to {
            let out = tick_q(e, fed, onset);
            if !out.is_empty() && first.is_none() {
                first = Some(fed);
            }
            all.extend(out);
            fed += CHUNK;
        }
        (all, first)
    }

    struct HardCuts {
        e: Endpointer,
        emits: Vec<Emit>,
        /// 每次 quietest 呼叫:(當次 fed, lo, hi)
        calls: Vec<(i64, i64, i64)>,
        last_fed: i64,
    }

    /// 連續語音不呼叫 on_small,onset 恆為 Some(0),從 5504 每塊 tick 到 192000。
    fn hard_cuts() -> HardCuts {
        let mut e = Endpointer::new(p());
        let mut emits = Vec::new();
        let mut calls = Vec::new();
        let mut fed = 5504;
        let mut last_fed = fed;
        while fed <= 192000 {
            let mut now = Vec::new();
            let out = e.tick(fed, Some(0), &mut |lo, hi| {
                now.push((lo, hi));
                hi - 1600
            });
            calls.extend(now.into_iter().map(|(lo, hi)| (fed, lo, hi)));
            emits.extend(out);
            last_fed = fed;
            fed += CHUNK;
        }
        HardCuts { e, emits, calls, last_fed }
    }

    // T1:組長 <2.5 s 要 base 停頓,>=2.5 s 為 0
    #[test]
    fn need_gap_switches_at_long_len() {
        let p = p();
        assert_eq!(need_gap(15840, &p), 4800, "0.99 s 的組應需要 base 停頓");
        assert_eq!(need_gap(39840, &p), 4800, "2.49 s 仍屬短組,需要 base");
        assert_eq!(need_gap(40000, &p), 0, "剛好 2.5 s 起視為長組,不需停頓");
    }

    // T2:長組遇到小段停頓立即送出,不等 base
    #[test]
    fn long_group_emits_right_at_small_gap() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 41600), 43200), None, "第一個小段只開組,不該送出");
        let out = tick_q(&mut e, 43200, None);
        assert_eq!(out.len(), 1, "長組在小段結束的那次 tick 就該送出");
        assert_eq!(out[0], Emit { core: (0, 41600), forced: false, post_limit: 1600 });
    }

    // T3:短組要等 base + onset_lag 才送出,不會提早也不會晚一塊以上
    #[test]
    fn short_group_emits_after_base_plus_lag() {
        let mut e0 = Endpointer::new(p());
        assert_eq!(e0.on_small((0, 12800), 14400), None);
        assert!(tick_q(&mut e0, 17600, None).is_empty(), "E+base 時盲區還沒過,不可送出");

        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 12800), 14400), None);
        let (all, first) = run_ticks(&mut e, 14400, 30000, None);
        assert!(first.is_some(), "短組最終必須送出");
        let first = first.unwrap_or(0);
        assert!(first >= 23104, "不可早於 E+base+L(23104),實際 {first}");
        assert!(first < 23104 + CHUNK, "不可晚過一塊,實際 {first}");
        assert_eq!(all.len(), 1, "只能送出一次");
        assert_eq!(all[0].core, (0, 12800));
        assert!(!all[0].forced);
        assert_eq!(all[0].post_limit, 3200, "靜音夠長時後補白取 max_post");
    }

    // T4:盲區內下一句開口(晚到偵測)要併入同組,不可先送出
    #[test]
    fn onset_detected_late_merges_into_group() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 12800), 14400), None);
        let mut all = Vec::new();
        let (a, _) = run_ticks(&mut e, 14400, 20703, None);
        all.extend(a);
        let (b, _) = run_ticks(&mut e, 20704, 31600, Some(15200));
        all.extend(b);
        if let Some(x) = e.on_small((15200, 30000), 31600) {
            all.push(x);
        }
        let (c, _) = run_ticks(&mut e, 32080, 60000, None);
        all.extend(c);
        assert_eq!(all.len(), 1, "兩句併成一組,只能送出一次,實際 {all:?}");
        assert_eq!(all[0].core, (0, 30000), "併組後 core 應涵蓋兩句");
    }

    // T5:偵測到開口且間隔 >= base,立即送出前組
    #[test]
    fn onset_after_base_gap_emits_previous_group() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 12800), 14400), None);
        let out = tick_q(&mut e, 24704, Some(19200));
        assert_eq!(out.len(), 1, "開口距組尾 6400 >= base,應立即送出前組");
        assert_eq!(out[0], Emit { core: (0, 12800), forced: false, post_limit: 3200 });
    }

    // T6:連續 12 s 不停,每約 4 s 硬切,片段首尾相接且不超過上限
    #[test]
    fn continuous_speech_is_hard_cut_at_max_len() {
        let h = hard_cuts();
        assert!(h.emits.len() >= 2, "12 s 連續語音至少切 2 次,實際 {}", h.emits.len());
        assert_eq!(h.emits[0].core.0, 0, "第一個切段從 0 開始");
        for (i, em) in h.emits.iter().enumerate() {
            assert!(em.forced, "第 {i} 段應為硬切");
            assert_eq!(em.post_limit, 0, "硬切不補白");
            assert!(em.core.1 - em.core.0 <= 64000, "第 {i} 段超過 max_len:{:?}", em.core);
            if i > 0 {
                assert_eq!(em.core.0, h.emits[i - 1].core.1, "第 {i} 段應接在前一段尾");
            }
        }
    }

    // T7:硬切後才交付的小段要依切點裁切,不重複送已切走的部分
    #[test]
    fn small_segment_after_hard_cut_is_trimmed() {
        let h = hard_cuts();
        assert!(!h.emits.is_empty(), "前置:必須已硬切");
        let c = h.emits.last().map(|x| x.core.1).unwrap_or(-1);
        let mut e = h.e;
        let mut all = Vec::new();
        if let Some(x) = e.on_small((0, 192000), 193600) {
            all.push(x);
        }
        let (rest, _) = run_ticks(&mut e, 193600, 260000, None);
        all.extend(rest);
        assert_eq!(all.len(), 1, "只剩切點後的一段,實際 {all:?}");
        assert!(!all[0].forced);
        assert_eq!(all[0].core, (c, 192000), "起點應裁到最後切點 c");

        // b <= c 的小段整個丟掉
        let h2 = hard_cuts();
        let c2 = h2.emits.last().map(|x| x.core.1).unwrap_or(-1);
        let mut e2 = h2.e;
        assert_eq!(e2.on_small((0, c2), h2.last_fed), None, "整段在切點之前,應丟棄");
        assert_eq!(e2.open_start(), Some(c2), "丟棄後開著的組仍從切點起");
    }

    // T8:硬切的安靜搜尋範圍 = [max(fed-24000, 組首+16000), fed)
    #[test]
    fn hard_cut_search_range_is_bounded() {
        let h = hard_cuts();
        assert!(!h.calls.is_empty(), "必須有呼叫 quietest");
        assert_eq!(h.calls.len(), h.emits.len(), "每次硬切恰好找一次切點");
        for (i, (fed, lo, hi)) in h.calls.iter().enumerate() {
            let head = if i == 0 { 0 } else { h.emits[i - 1].core.1 };
            assert_eq!(*hi, *fed, "第 {i} 次搜尋上界應為 fed");
            assert_eq!(*lo, (*fed - 24000).max(head + 16000), "第 {i} 次搜尋下界錯誤");
        }
    }

    // T9:quietest_frame 取最小 RMS 框的中點、同值取最後、不足一框取中點
    #[test]
    fn quietest_frame_picks_min_rms_frame() {
        let mut x = vec![0.5f32; 960];
        x[320..640].iter_mut().for_each(|v| *v = 0.0);
        assert_eq!(quietest_frame(&x, 1000, 320), 1000 + 480, "第二框全 0,中點為 origin+480");
        let flat = vec![0.5f32; 960];
        assert_eq!(quietest_frame(&flat, 1000, 320), 1000 + 800, "全相同時取最後一框中點");
        let short = vec![0.5f32; 100];
        assert_eq!(quietest_frame(&short, 1000, 320), 1000 + 50, "不足一框取中點");
    }

    // T10:非強制送出的後補白不會吃到 120 ms 後開口的下一句
    #[test]
    fn post_limit_does_not_eat_next_utterance() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 41600), 43200), None);
        let out = tick_q(&mut e, 43200, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].post_limit, 1600);
        assert!(out[0].post_limit <= 1920, "後補白不可超過 E+120 ms");
    }

    // T11:set_params 後下一次 tick 立即套用新門檻與上限
    #[test]
    fn set_params_takes_effect_immediately() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 12800), 14400), None);
        assert!(tick_q(&mut e, 20000, None).is_empty(), "舊門檻下 7200 < 10304,不送");
        e.set_params(1600, 64000);
        let out = tick_q(&mut e, 20480, None);
        assert_eq!(out.len(), 1, "新門檻 1600+5504=7104 <= 7680,應送出");
        assert_eq!(out[0].core, (0, 12800));

        let mut e = Endpointer::new(EndpointParams::new(300, 6000, 250));
        let (early, _) = run_ticks(&mut e, 5504, 80000, Some(0));
        assert!(early.is_empty(), "上限 96000 時 80000 以內不該切,實際 {early:?}");
        e.set_params(4800, 64000);
        let out = tick_q(&mut e, 80480, Some(0));
        assert_eq!(out.len(), 1, "上限降為 64000 後,組長 80480 應立即硬切");
        assert!(out[0].forced);
    }

    // T12:孤立的 150 ms 雜訊組丟掉不送,剛好 250 ms 則保留
    #[test]
    fn isolated_noise_group_is_dropped() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 2400), 4000), None);
        assert_eq!(e.open_start(), Some(0), "雜訊小段先開組,等時間到才判斷丟棄");
        let (all, _) = run_ticks(&mut e, 4000, 40000, None);
        assert!(all.is_empty(), "150 ms 雜訊不可送出,實際 {all:?}");
        assert_eq!(e.open_start(), None, "丟棄後不留開著的組");
        assert_eq!(e.finish_group(40000), None);

        let mut k = Endpointer::new(p());
        assert_eq!(k.on_small((0, 4000), 5600), None);
        let (kept, _) = run_ticks(&mut k, 5600, 40000, None);
        assert_eq!(kept.len(), 1, "剛好 min_keep 不算 <,應保留");
        assert_eq!(kept[0].core, (0, 4000));
    }

    // T13:停止或切換來源時 finish_group 送出開著的組並清空
    #[test]
    fn finish_group_flushes_and_clears() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.on_small((0, 12800), 14400), None);
        let out = e.finish_group(15000);
        assert!(out.is_some(), "組尾已知,應送出");
        let out = out.unwrap_or(Emit { core: (-1, -1), forced: true, post_limit: -1 });
        assert_eq!(out.core, (0, 12800));
        assert!(!out.forced);
        assert_eq!(e.open_start(), None, "送出後狀態清空");

        let mut f = Endpointer::new(p());
        assert_eq!(f.finish_group(0), None, "沒有組時回 None");

        let mut h = hard_cuts();
        assert!(!h.emits.is_empty(), "前置:必須已硬切");
        assert_eq!(h.e.finish_group(h.last_fed), None, "硬切後組尾未知,回 None");
        assert_eq!(h.e.open_start(), None, "仍須清空狀態");
    }

    // T14:speaking 在偵測中、組開著、最後小段 5504 內為 true,之後為 false
    #[test]
    fn speaking_covers_detection_open_group_and_lag() {
        let mut e = Endpointer::new(p());
        assert!(e.speaking(0, true), "VAD 偵測中必為 true");

        assert_eq!(e.on_small((0, 12800), 14400), None);
        assert!(e.speaking(14400, false), "組開著視為還在講");

        let mut l = Endpointer::new(p());
        assert_eq!(l.on_small((0, 41600), 43200), None);
        assert_eq!(tick_q(&mut l, 43200, None).len(), 1);
        assert!(l.speaking(43200, false), "送出後距小段結尾 1600 < 5504 仍算在講");

        let mut s = Endpointer::new(p());
        assert_eq!(s.on_small((0, 12800), 14400), None);
        let (all, _) = run_ticks(&mut s, 14400, 23584, None);
        assert_eq!(all.len(), 1, "前置:T3 規格的送出格(< 23584)應已送出");
        assert!(!s.speaking(23584, false), "距小段結尾 10784 >= 5504,不再算在講");
    }

    // T20:硬切時語音剛結束、之後沒有新小段 → 組被清掉,不會把靜音當 forced 段送出
    #[test]
    fn hard_cut_without_followup_small_clears_group() {
        let mut e = Endpointer::new(p());
        let mut emits = Vec::new();
        let mut fed = 5504;
        while fed <= 64320 {
            emits.extend(e.tick(fed, Some(0), &mut |_, _| 64000));
            fed += CHUNK;
        }
        assert_eq!(emits.len(), 1, "前置:撞上限硬切一次");
        assert_eq!(e.on_small((0, 63800), 64800), None, "小段整段在切點前,丟掉");
        while fed <= 200_000 {
            emits.extend(e.tick(fed, None, &mut |_, hi| hi - 1600));
            fed += CHUNK;
        }
        assert_eq!(emits.len(), 1, "靜音期間不得再送出段落,實際 {emits:?}");
        assert_eq!(e.open_start(), None, "沒有語音的殘組要清掉");
        assert!(!e.speaking(200_000, false), "殘組不能讓 speaking 一直為 true");
        let out = e.tick(200_480, Some(190_000), &mut |_, hi| hi - 1600);
        assert!(out.is_empty());
        assert_eq!(e.open_start(), Some(190_000), "下一句另開新組,不接在舊切點");
    }

    // T21:手動上限 2 s(小於 long_len)時硬切點落在組內,不會切出組尾之後的靜音
    #[test]
    fn hard_cut_without_onset_stays_inside_group() {
        let mut e = Endpointer::new(EndpointParams::new(300, 2000, 250));
        assert_eq!(e.on_small((0, 36000), 37600), None);
        let mut emits = Vec::new();
        let mut fed = 37600;
        while fed <= 120_000 {
            emits.extend(e.tick(fed, None, &mut |lo, hi| {
                assert!(hi <= 36000, "沒有 onset 時搜尋範圍不得超過組尾,實際 hi={hi}");
                (lo + hi) / 2
            }));
            fed += CHUNK;
        }
        assert!(!emits.is_empty(), "組應送出");
        assert!(emits.iter().all(|m| m.core.1 <= 36000), "不得送出組尾之後的靜音,實際 {emits:?}");
        assert_eq!(emits.last().map(|m| m.core.1), Some(36000), "最後一段收在組尾");
        assert_eq!(e.open_start(), None);
    }

    // T15:open_start 在開組時為組首、硬切後為最後切點、送出後為 None
    #[test]
    fn open_start_tracks_group_head() {
        let mut e = Endpointer::new(p());
        assert_eq!(e.open_start(), None, "尚未有組");
        assert_eq!(e.on_small((3200, 12800), 14400), None);
        assert_eq!(e.open_start(), Some(3200), "組開著時回組首");

        let h = hard_cuts();
        let c = h.emits.last().map(|x| x.core.1);
        assert!(c.is_some(), "前置:必須已硬切");
        assert_eq!(h.e.open_start(), c, "硬切後組首是最後切點");

        let (all, _) = run_ticks(&mut e, 14400, 30000, None);
        assert_eq!(all.len(), 1, "前置:短組應已送出");
        assert_eq!(e.open_start(), None, "送出後無開著的組");
    }
}
