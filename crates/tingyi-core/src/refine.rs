//! 回修(慢路徑):快路徑先顯示,幾秒後用更長的連續音訊重新辨識、依時間對齊取代原文、
//! 補回 VAD 漏掉的語音,再帶前後文重新翻譋。
//!
//! 執行模型:
//! - `refine_loop`(std thread):擁有 `AudioRing`(連續音訊)與 `Ledger`(快路徑句子帳本),
//!   只透過 `RefineMsg` 接收資料,從不碰快路徑狀態;每 250 ms 以 `pick_window` 評估是否有可回修的視窗。
//! - `refine_mt_loop`(tokio):收 `RefineJob`,等快路徑翻譋佇列空了才送,用 llama-server 第二個 slot。
//!
//! 純函式 `align` / `pick_window` / `normalize_for_compare` 可單元測試。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tokio::sync::mpsc;

use crate::asr::{detect_lang, Lang, TimedSegment, WhisperServer};
use crate::audio::normalize_rms;
use crate::audio::ring::{ms_to_samples, samples_to_ms, AudioRing};
use crate::metrics::PipelineStats;
use crate::mt::{TranslateRequest, Translator};
use crate::pipeline::{PipelineConfig, PipelineEvent, SharedSpeakers, NEXT_SENTENCE_ID};
use crate::postprocess::{postprocess, GlossaryEntry};

#[derive(Debug, Clone)]
pub struct RefineConfig {
    /// 預設開;只有 Whisper 引擎有時間戳,SenseVoice 會在啟動時被關掉。
    pub enabled: bool,
    pub ring_secs: u32,
    /// 視窗累積到這麼長就觸發。
    pub min_window_ms: u32,
    /// 視窗上限(≤ Whisper 單一 30 s 視窗,避免 seek 分塊造成時間戳不連續)。
    pub max_window_ms: u32,
    /// 講者靜音這麼久且視窗內 ≥ 2 句也觸發。
    pub silence_trigger_ms: u32,
    /// 視窗尾端離「現在」至少這麼遠,避免跟還在講、還沒辨識完的段落重疊。
    pub tail_margin_ms: u32,
    /// Whisper segment 中點落在句區間 ± 這個容忍內就算該句的。
    pub align_tol_ms: u32,
    /// 相鄰無主 segment 間隔小於此就併成同一個補回句。
    pub recover_merge_gap_ms: u32,
    /// 等視窗內句子翻譋完成的上限;超過就不等了。
    pub settle_timeout_ms: u32,
    /// 回修翻譋帶的前文句數(只取有譯文的)。
    pub context_sentences: usize,
    /// 回修翻譋帶的後文句數。
    pub lookahead_sentences: usize,
    /// None = 與快路徑共用同一個 whisper-server;Some = 獨立伺服器。
    pub whisper_url: Option<String>,
}

impl Default for RefineConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ring_secs: 90,
            min_window_ms: 8000,
            max_window_ms: 25_000,
            silence_trigger_ms: 1500,
            tail_margin_ms: 1500,
            align_tol_ms: 300,
            recover_merge_gap_ms: 500,
            settle_timeout_ms: 10_000,
            context_sentences: 3,
            lookahead_sentences: 2,
            whisper_url: None,
        }
    }
}

/// 快路徑 → refine thread。
pub enum RefineMsg {
    /// vad thread 每個 chunk(與 Segmenter 同一批樣本)。
    Audio(Vec<f32>),
    /// asr thread 正式辨識完一句(不論是否翻譋)。
    Sentence {
        id: u64,
        start: i64,
        end: i64,
        text: String,
        lang: Lang,
        speaker: Option<String>,
        will_translate: bool,
    },
    /// mt task 把 `from` 併進 `into`。
    Merged { into: u64, from: u64, text: String, start: i64, end: i64 },
    /// 快路徑或回修翻譋完成。
    Translated { id: u64, text: String },
    /// 翻譋失敗/逾時(視為已結案)。
    Failed { id: u64 },
    /// 音訊來源在此切換(緊接在切換靜音之後送出):回修不跨過這一點。
    Boundary,
}

/// refine thread → refine_mt task。
pub struct RefineJob {
    pub id: u64,
    pub text: String,
    pub lang: Lang,
    pub context: Vec<(String, String)>,
    pub lookahead: Vec<String>,
    pub glossary: Vec<GlossaryEntry>,
}

/// 帳本上的一句(快路徑或補回)。
#[derive(Debug, Clone)]
pub struct Ledger {
    pub id: u64,
    pub start: i64,
    pub end: i64,
    pub text: String,
    pub lang: Lang,
    pub speaker: Option<String>,
    pub will_translate: bool,
    pub translation: Option<String>,
    /// 翻譋完成 / 失敗 / 不翻 → true。
    pub settled: bool,
    pub seen_at: Instant,
}

impl Ledger {
    fn settled_or_timeout(&self, now: Instant, timeout: Duration) -> bool {
        self.settled || now.duration_since(self.seen_at) >= timeout
    }
}

/// 對齊用的句區間。
#[derive(Debug, Clone)]
pub struct Span {
    pub id: u64,
    pub start: i64,
    pub end: i64,
    pub text: String,
    pub lang: Lang,
}

/// Whisper 回的一段(秒,相對視窗起點)。
#[derive(Debug, Clone)]
pub struct WSeg {
    pub start_s: f32,
    pub end_s: f32,
    pub text: String,
}

impl From<&TimedSegment> for WSeg {
    fn from(t: &TimedSegment) -> Self {
        Self { start_s: t.start_s, end_s: t.end_s, text: t.text.clone() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    /// 插在這句之後;None = 視窗第一句之前。
    pub after: Option<u64>,
    pub start: i64,
    pub end: i64,
    pub text: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct AlignOutcome {
    pub revised: Vec<(u64, String)>,
    pub recovered: Vec<Recovered>,
}

/// 比較用正規化:去空白、中英標點,全形英數→半形,ASCII 小寫。
pub fn normalize_for_compare(s: &str) -> String {
    const PUNCT: &str = "。、,,.!?!?…「」『』・〜～~:;:;\"'()()[]【】<>《》—-‥";
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        // 先全形→半形,再判斷標點(全形「?」「!」等也要一起去掉)
        let c = match c as u32 {
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFF01 + 0x21).unwrap_or(c),
            _ => c,
        };
        if c.is_whitespace() || PUNCT.contains(c) {
            continue;
        }
        for l in c.to_lowercase() {
            out.push(l);
        }
    }
    out
}

fn cjk_chars(s: &str) -> usize {
    s.chars().filter(|c| {
        let u = *c as u32;
        (0x3040..=0x30FF).contains(&u) || (0x4E00..=0x9FFF).contains(&u) || (0xAC00..=0xD7AF).contains(&u) || (0x3400..=0x4DBF).contains(&u)
    }).count()
}

fn join_texts(parts: &[&str], lang: Lang) -> String {
    let sep = if lang.is_cjk() { "" } else { " " };
    let joined = parts.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(sep);
    crate::asr::collapse_cjk_spaces(&joined).trim().to_string()
}

/// 依時間把 Whisper segments 分派給既有句子;落在句外的成為補回句。
pub fn align(window_start: i64, sentences: &[Span], segs: &[WSeg], cfg: &RefineConfig) -> AlignOutcome {
    const SR: f32 = 16000.0;
    let tol = ms_to_samples(cfg.align_tol_ms as u64);
    struct Placed { a: i64, b: i64, text: String }
    let placed: Vec<Placed> = segs
        .iter()
        .filter(|s| !s.text.trim().is_empty() && !crate::asr::is_hallucination(&s.text))
        .map(|s| Placed {
            a: window_start + (s.start_s * SR).round() as i64,
            b: window_start + (s.end_s * SR).round() as i64,
            text: s.text.trim().to_string(),
        })
        .collect();

    let mut buckets: Vec<Vec<&Placed>> = vec![Vec::new(); sentences.len()];
    let mut unassigned: Vec<&Placed> = Vec::new();
    for p in &placed {
        // 一個 segment 明顯橫跨兩句以上(各重疊 ≥ 該句 30% 或 500 ms):沒有逐字時間戳切不開,
        // 拿它修任何一句都會把鄰句內容吃進來 → 保守略過。
        let spanned = sentences
            .iter()
            .filter(|s| {
                let ov = (p.b.min(s.end) - p.a.max(s.start)).max(0);
                let need = ((s.end - s.start) * 3 / 10).min(ms_to_samples(500)).max(1);
                ov >= need
            })
            .count();
        if spanned >= 2 {
            continue;
        }
        let mid = (p.a + p.b) / 2;
        let cands: Vec<usize> = sentences
            .iter()
            .enumerate()
            .filter(|(_, s)| s.start - tol <= mid && mid <= s.end + tol)
            .map(|(i, _)| i)
            .collect();
        match cands.len() {
            0 => unassigned.push(p),
            1 => buckets[cands[0]].push(p),
            _ => {
                // 重疊最大者;同分取較早句
                let best = cands
                    .iter()
                    .copied()
                    .max_by_key(|&i| {
                        let s = &sentences[i];
                        let ov = (p.b.min(s.end) - p.a.max(s.start)).max(0);
                        (ov, std::cmp::Reverse(s.start))
                    })
                    .unwrap();
                buckets[best].push(p);
            }
        }
    }

    let mut out = AlignOutcome::default();
    for (i, group) in buckets.iter().enumerate() {
        if group.is_empty() {
            continue; // 沒有任何 segment 落在句內 → 不動,絕不清空
        }
        let mut g = group.clone();
        g.sort_by_key(|p| p.a);
        let parts: Vec<&str> = g.iter().map(|p| p.text.as_str()).collect();
        let new = join_texts(&parts, sentences[i].lang);
        if !new.is_empty() && normalize_for_compare(&new) != normalize_for_compare(&sentences[i].text) {
            out.revised.push((sentences[i].id, new));
        }
    }

    // 補回:無主 segment 依時間排序,相鄰併組
    unassigned.sort_by_key(|p| p.a);
    let gap = ms_to_samples(cfg.recover_merge_gap_ms as u64);
    let mut groups: Vec<Vec<&Placed>> = Vec::new();
    for p in unassigned {
        match groups.last_mut() {
            Some(g) if p.a - g.last().unwrap().b <= gap => g.push(p),
            _ => groups.push(vec![p]),
        }
    }
    let min_dur = ms_to_samples(300);
    for g in groups {
        let a = g.first().unwrap().a;
        let b = g.last().unwrap().b;
        if b - a < min_dur {
            continue;
        }
        let parts: Vec<&str> = g.iter().map(|p| p.text.as_str()).collect();
        let lang_guess = detect_lang(&parts.join(" "));
        let text = join_texts(&parts, lang_guess);
        let norm = normalize_for_compare(&text);
        if norm.is_empty() {
            continue;
        }
        let enough = if lang_guess.is_cjk() { cjk_chars(&text) >= 2 } else { text.split_whitespace().count() >= 1 };
        if !enough {
            continue;
        }
        // 鄰句(前一句、後一句)已含相同文字 → 時間戳偏移造成的重複,丟棄
        let prev = sentences.iter().filter(|s| s.end <= a).max_by_key(|s| s.end);
        let next = sentences.iter().filter(|s| s.start >= b).min_by_key(|s| s.start);
        let dup = [prev, next].iter().flatten().any(|s| {
            let n = normalize_for_compare(&s.text);
            !n.is_empty() && (n == norm || n.contains(&norm))
        });
        if dup {
            continue;
        }
        out.recovered.push(Recovered { after: prev.map(|s| s.id), start: a, end: b, text });
    }
    out
}

/// 決定下一個回修視窗 `[a, b)`;None = 還不該觸發。
#[allow(clippy::too_many_arguments)]
pub fn pick_window(
    ledger: &[Ledger],
    refined_until: i64,
    ring_start: i64,
    now_sample: i64,
    silence_ms: u64,
    now: Instant,
    cfg: &RefineConfig,
) -> Option<(i64, i64)> {
    let a = refined_until.max(ring_start);
    let tail_limit = now_sample - ms_to_samples(cfg.tail_margin_ms as u64);
    let cands: Vec<&Ledger> = ledger.iter().filter(|l| l.start >= a && l.end <= tail_limit).collect();
    if cands.is_empty() {
        return None;
    }
    let max_len = ms_to_samples(cfg.max_window_ms as u64);
    let last_end = cands.iter().map(|l| l.end).max().unwrap();
    let (b, forced) = if last_end - a > max_len {
        // 上限:切在句界
        let b = cands.iter().filter(|l| l.end <= a + max_len).map(|l| l.end).max();
        match b {
            Some(b) => (b, true),
            None => (cands.iter().map(|l| l.end).min().unwrap(), true), // 單句就超過上限,仍整句處理
        }
    } else {
        (last_end, false)
    };
    let in_window: Vec<&Ledger> = cands.iter().copied().filter(|l| l.end <= b).collect();
    let len = b - a;
    if !forced {
        let long_enough = len >= ms_to_samples(cfg.min_window_ms as u64);
        let quiet = silence_ms >= cfg.silence_trigger_ms as u64 && in_window.len() >= 2 && len >= ms_to_samples(2000);
        if !long_enough && !quiet {
            return None;
        }
    }
    let timeout = Duration::from_millis(cfg.settle_timeout_ms as u64);
    if !in_window.iter().all(|l| l.settled_or_timeout(now, timeout)) {
        return None;
    }
    Some((a, b))
}

/// refine thread 主迴圈。
#[allow(clippy::too_many_arguments)]
pub(crate) fn refine_loop(
    cfg: PipelineConfig,
    asr: WhisperServer,
    speakers: SharedSpeakers,
    stats: Arc<PipelineStats>,
    rx: Receiver<RefineMsg>,
    job_tx: Option<mpsc::UnboundedSender<RefineJob>>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    stop: Arc<AtomicBool>,
) {
    let rc = cfg.refine.clone();
    let mut ring = AudioRing::with_seconds(rc.ring_secs);
    let mut ledger: Vec<Ledger> = Vec::new();
    let mut refined_until: i64 = 0;
    let mut last_tail: Option<String> = None;
    let mut last_tick = Instant::now();

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(RefineMsg::Audio(s)) => ring.push(&s),
            Ok(RefineMsg::Sentence { id, start, end, text, lang, speaker, will_translate }) => {
                let l = Ledger {
                    id,
                    start,
                    end,
                    text,
                    lang,
                    speaker,
                    will_translate,
                    translation: None,
                    settled: !will_translate,
                    seen_at: Instant::now(),
                };
                insert_sorted(&mut ledger, l);
            }
            Ok(RefineMsg::Merged { into, from, text, start, end }) => {
                ledger.retain(|l| l.id != from);
                if let Some(l) = ledger.iter_mut().find(|l| l.id == into) {
                    l.text = text;
                    l.start = start;
                    l.end = end;
                }
                ledger.sort_by_key(|l| l.start);
            }
            Ok(RefineMsg::Translated { id, text }) => {
                if let Some(l) = ledger.iter_mut().find(|l| l.id == id) {
                    l.translation = Some(text);
                    l.settled = true;
                }
            }
            Ok(RefineMsg::Failed { id }) => {
                if let Some(l) = ledger.iter_mut().find(|l| l.id == id) {
                    l.settled = true;
                }
            }
            // 切換點之前尚未回修的句子放棄回修:跨來源的視窗會把兩邊的聲音拼在一起辨識
            Ok(RefineMsg::Boundary) => refined_until = refined_until.max(ring.end()),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if last_tick.elapsed() < Duration::from_millis(250) {
            continue;
        }
        last_tick = Instant::now();
        let ring_start = ring.start();
        // 回修停滯超過緩衝長度 → 放棄更早的部分
        if refined_until < ring_start {
            refined_until = ring_start;
        }
        ledger.retain(|l| l.end > ring_start - ms_to_samples(60_000)); // 帳本只留最近約 2.5 分鐘
        let Some((a, b)) = pick_window(&ledger, refined_until, ring_start, ring.end(), stats.silence_ms(), Instant::now(), &rc) else {
            continue;
        };
        refined_until = b;
        let Some(pcm) = ring.slice(a, b) else { continue };
        let pcm = if cfg.normalize_audio { normalize_rms(&pcm, -20.0, 8.0) } else { pcm };

        let spans: Vec<Span> = ledger
            .iter()
            .filter(|l| l.start >= a && l.end <= b)
            .map(|l| Span { id: l.id, start: l.start, end: l.end, text: l.text.clone(), lang: l.lang })
            .collect();
        let majority = majority_lang(&spans);
        let hint = cfg.lock_lang.or(majority);
        let t0 = Instant::now();
        let timed = match asr.transcribe_timestamped(&pcm, hint, last_tail.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("回修辨識失敗:{e:#}");
                continue;
            }
        };
        stats.record_refine(t0.elapsed().as_millis() as u32);
        let segs: Vec<WSeg> = timed.segments.iter().map(WSeg::from).collect();
        let outcome = align(a, &spans, &segs, &rc);
        tracing::info!(
            "回修視窗 {:.1}s–{:.1}s:{} 句、{} segments → 修 {} 補 {}",
            samples_to_ms(a) as f32 / 1000.0,
            samples_to_ms(b) as f32 / 1000.0,
            spans.len(),
            segs.len(),
            outcome.revised.len(),
            outcome.recovered.len()
        );

        for (id, text) in outcome.revised {
            // 與快路徑同一套清理,否則會把取代表修好的字換回錯字
            let Some(text) = crate::pipeline::clean(&cfg, text).filter(|t| !t.is_empty()) else { continue };
            if ledger.iter().any(|l| l.id == id && l.text == text) {
                continue;
            }
            if let Some(l) = ledger.iter_mut().find(|l| l.id == id) {
                l.text = text.clone();
            }
            PipelineStats::add(&stats.revised_sources, 1);
            let _ = events.send(PipelineEvent::SourceRevised { id, text });
            if let Some(job) = make_job(&ledger, id, &cfg, &rc) {
                if let Some(tx) = &job_tx {
                    let _ = tx.send(job);
                }
            }
        }
        for mut r in outcome.recovered {
            let Some(text) = crate::pipeline::clean(&cfg, std::mem::take(&mut r.text)).filter(|t| !t.is_empty()) else { continue };
            r.text = text;
            let id = NEXT_SENTENCE_ID.fetch_add(1, Ordering::Relaxed);
            let mut lang = detect_lang(&r.text);
            if lang == Lang::Unknown {
                lang = hint.unwrap_or(Lang::Unknown);
            }
            let speaker = ring
                .slice(r.start, r.end)
                .and_then(|p| speakers.lock().unwrap().as_ref().and_then(|s| s.peek(&normalize_rms(&p, -20.0, 8.0))));
            let will_translate = lang != Lang::Zh && (cfg.allowed_langs.is_empty() || cfg.allowed_langs.contains(&lang));
            PipelineStats::add(&stats.recovered, 1);
            let _ = events.send(PipelineEvent::Recovered {
                id,
                after: r.after,
                text: r.text.clone(),
                lang,
                speaker: speaker.clone(),
                start_ms: samples_to_ms(r.start),
                end_ms: samples_to_ms(r.end),
                will_translate,
            });
            insert_sorted(
                &mut ledger,
                Ledger {
                    id,
                    start: r.start,
                    end: r.end,
                    text: r.text,
                    lang,
                    speaker,
                    will_translate,
                    translation: None,
                    settled: !will_translate,
                    seen_at: Instant::now(),
                },
            );
            if will_translate {
                if let (Some(job), Some(tx)) = (make_job(&ledger, id, &cfg, &rc), &job_tx) {
                    let _ = tx.send(job);
                }
            }
        }
        last_tail = ledger.iter().filter(|l| l.end <= b).max_by_key(|l| l.end).map(|l| l.text.clone());
    }
    tracing::info!("refine thread 結束");
}

fn insert_sorted(ledger: &mut Vec<Ledger>, l: Ledger) {
    let pos = ledger.iter().position(|x| x.start > l.start).unwrap_or(ledger.len());
    ledger.insert(pos, l);
}

fn majority_lang(spans: &[Span]) -> Option<Lang> {
    let mut counts: Vec<(Lang, usize)> = Vec::new();
    for s in spans {
        if s.lang == Lang::Unknown {
            continue;
        }
        match counts.iter_mut().find(|(l, _)| *l == s.lang) {
            Some(c) => c.1 += 1,
            None => counts.push((s.lang, 1)),
        }
    }
    counts.into_iter().max_by_key(|c| c.1).map(|c| c.0)
}

fn make_job(ledger: &[Ledger], id: u64, cfg: &PipelineConfig, rc: &RefineConfig) -> Option<RefineJob> {
    let idx = ledger.iter().position(|l| l.id == id)?;
    let l = &ledger[idx];
    if !l.will_translate {
        return None;
    }
    let context: Vec<(String, String)> = ledger[..idx]
        .iter()
        .rev()
        .filter_map(|x| x.translation.as_ref().map(|t| (x.text.clone(), t.clone())))
        .take(rc.context_sentences)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let lookahead: Vec<String> = ledger[idx + 1..].iter().take(rc.lookahead_sentences).map(|x| x.text.clone()).collect();
    let glossary = cfg.glossary.hits(&l.text).into_iter().cloned().collect();
    Some(RefineJob { id, text: l.text.clone(), lang: l.lang, context, lookahead, glossary })
}

/// 回修翻譋:等快路徑佇列空了才送,用第二個 slot;失敗只記 log,保留快譯。
pub(crate) async fn refine_mt_loop(
    cfg: PipelineConfig,
    translator: Arc<dyn Translator>,
    mut jobs: mpsc::UnboundedReceiver<RefineJob>,
    ledger_tx: Sender<RefineMsg>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    stats: Arc<PipelineStats>,
) {
    let mut backlog: VecDeque<RefineJob> = VecDeque::new();
    loop {
        let job = match backlog.pop_front() {
            Some(j) => j,
            None => match jobs.recv().await {
                Some(j) => j,
                None => break,
            },
        };
        // 同一句若有更新的工作排在後面,先吸進來只翻最後一版
        while let Ok(j) = jobs.try_recv() {
            backlog.push_back(j);
        }
        if backlog.iter().any(|j| j.id == job.id) {
            continue;
        }
        let waited = Instant::now();
        while stats.mt_queue() > 0 && waited.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let req = TranslateRequest {
            text: job.text.clone(),
            src_lang: job.lang,
            context: job.context.clone(),
            glossary: job.glossary.clone(),
            lookahead: job.lookahead.clone(),
            slot: Some(crate::translate_server::REFINE_SLOT),
        };
        let mut noop = |_: &str| {};
        match tokio::time::timeout(cfg.mt_timeout * 2, translator.translate(req, &mut noop)).await {
            Ok(Ok(res)) => {
                let hits: Vec<&GlossaryEntry> = job.glossary.iter().collect();
                let text = postprocess(&res.text, &hits);
                if text.is_empty() {
                    continue;
                }
                PipelineStats::add(&stats.refine_mt_done, 1);
                let _ = events.send(PipelineEvent::TranslationRevised { id: job.id, text: text.clone() });
                let _ = ledger_tx.send(RefineMsg::Translated { id: job.id, text });
            }
            Ok(Err(e)) => tracing::warn!("回修翻譋失敗 #{}:{e:#}", job.id),
            Err(_) => tracing::warn!("回修翻譋逾時 #{}", job.id),
        }
    }
    tracing::info!("refine mt task 結束");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RefineConfig {
        RefineConfig::default()
    }
    fn s(id: u64, a_ms: u64, b_ms: u64, text: &str) -> Span {
        Span { id, start: ms_to_samples(a_ms), end: ms_to_samples(b_ms), text: text.into(), lang: Lang::Ja }
    }
    fn w(a: f32, b: f32, t: &str) -> WSeg {
        WSeg { start_s: a, end_s: b, text: t.into() }
    }

    #[test]
    fn normalize_for_compare_cases() {
        assert_eq!(normalize_for_compare("Ｈｅｌｌｏ, World!"), "helloworld");
        assert_eq!(normalize_for_compare("今日は、配信 ありがとう。"), "今日は配信ありがとう");
        assert_eq!(normalize_for_compare("「嘘でしょ?」"), "嘘でしょ");
    }

    #[test]
    fn align_assigns_by_midpoint() {
        let sents = [s(1, 0, 3000, "これね"), s(2, 3500, 7000, "ブーバー")];
        let segs = [w(0.1, 2.9, "ところねー"), w(3.6, 6.9, "シャールは12ボックスに1枚")];
        let o = align(0, &sents, &segs, &cfg());
        assert_eq!(o.revised, vec![(1, "ところねー".to_string()), (2, "シャールは12ボックスに1枚".to_string())]);
        assert!(o.recovered.is_empty());
    }

    #[test]
    fn align_unchanged_after_normalize() {
        let sents = [s(1, 0, 3000, "嘘でしょ?")];
        let segs = [w(0.2, 2.8, "嘘でしょ？")];
        let o = align(0, &sents, &segs, &cfg());
        assert!(o.revised.is_empty());
    }

    #[test]
    fn align_tolerance_edge_and_never_blanks() {
        let sents = [s(1, 0, 3000, "甲"), s(2, 5000, 8000, "乙")];
        // 中點 3.1 s,句尾 +100 ms 容忍內 → 併入第一句;第二句沒 segment → 不動
        let segs = [w(2.9, 3.3, "甲乙丙")];
        let o = align(0, &sents, &segs, &cfg());
        assert_eq!(o.revised, vec![(1, "甲乙丙".to_string())]);
    }

    #[test]
    fn align_recovers_gap_speech_and_merges_adjacent() {
        let sents = [s(1, 0, 3000, "甲甲"), s(2, 10_000, 13_000, "乙乙")];
        // 7.5–9 s 無句;兩個 segment 間隔 200 ms → 併成一句
        let segs = [w(0.1, 2.9, "甲甲"), w(7.5, 8.2, "漏掉的"), w(8.4, 9.0, "那一句"), w(10.1, 12.9, "乙乙")];
        let o = align(0, &sents, &segs, &cfg());
        assert!(o.revised.is_empty());
        assert_eq!(o.recovered.len(), 1);
        assert_eq!(o.recovered[0].text, "漏掉的那一句");
        assert_eq!(o.recovered[0].after, Some(1));
    }

    #[test]
    fn align_rejects_duplicate_recovered_and_hallucination() {
        let sents = [s(1, 0, 3000, "ご視聴ありがとう今日も")];
        // 鄰句已包含 → 丟;幻聽 → 丟;太短(< 300 ms)→ 丟
        let segs = [w(3.5, 4.5, "今日も"), w(5.0, 6.0, "ご視聴ありがとうございました"), w(7.0, 7.1, "あ")];
        let o = align(0, &sents, &segs, &cfg());
        assert!(o.recovered.is_empty(), "{:?}", o.recovered);
    }

    #[test]
    fn align_skips_segment_spanning_two_sentences() {
        let sents = [s(7, 0, 3200, "Hold on, did that boss just one-shot me?"), s(8, 3400, 5200, "That is actually insane.")];
        // Whisper 把兩句併成一段 0.1–5.1 s → 兩句都不該被改
        let segs = [w(0.1, 5.1, "Hold on, did that boss just one shot me? That is actually insane.")];
        let o = align(0, &sents, &segs, &cfg());
        assert!(o.revised.is_empty(), "{:?}", o.revised);
        assert!(o.recovered.is_empty());
    }

    #[test]
    fn align_recovered_before_first_sentence() {
        let sents = [s(1, 5000, 8000, "後句")];
        let segs = [w(0.5, 2.0, "最前面"), w(5.1, 7.9, "後句")];
        let o = align(0, &sents, &segs, &cfg());
        assert_eq!(o.recovered.len(), 1);
        assert_eq!(o.recovered[0].after, None);
    }

    fn led(id: u64, a_ms: u64, b_ms: u64, settled: bool) -> Ledger {
        Ledger {
            id,
            start: ms_to_samples(a_ms),
            end: ms_to_samples(b_ms),
            text: "x".into(),
            lang: Lang::Ja,
            speaker: None,
            will_translate: true,
            translation: None,
            settled,
            seen_at: Instant::now(),
        }
    }

    #[test]
    fn pick_window_rules() {
        let c = cfg();
        let now = Instant::now();
        let s = |ms: u64| ms_to_samples(ms);
        // 8 s 觸發
        let l = vec![led(1, 0, 4000, true), led(2, 4500, 8500, true)];
        assert_eq!(pick_window(&l, 0, 0, s(20_000), 0, now, &c), Some((0, s(8500))));
        // 不到 8 s 且不靜音 → 不觸發
        let l = vec![led(1, 0, 4000, true)];
        assert_eq!(pick_window(&l, 0, 0, s(20_000), 0, now, &c), None);
        // 靜音 1.5 s + 2 句 → 觸發
        let l = vec![led(1, 0, 1500, true), led(2, 1600, 3000, true)];
        assert_eq!(pick_window(&l, 0, 0, s(20_000), 2000, now, &c), Some((0, s(3000))));
        // 靜音但只有 1 句 → 不觸發
        let l = vec![led(1, 0, 3000, true)];
        assert_eq!(pick_window(&l, 0, 0, s(20_000), 2000, now, &c), None);
        // 尾端安全邊界:最後一句結束在 now − 1 s 內 → 排除,剩下不足 8 s
        let l = vec![led(1, 0, 4000, true), led(2, 4500, 9500, true)];
        assert_eq!(pick_window(&l, 0, 0, s(10_000), 0, now, &c), None);
        // 未 settled → 不觸發
        let l = vec![led(1, 0, 4000, true), led(2, 4500, 8500, false)];
        assert_eq!(pick_window(&l, 0, 0, s(20_000), 0, now, &c), None);
        // 25 s 上限切在句界
        let l = vec![led(1, 0, 10_000, true), led(2, 10_500, 20_000, true), led(3, 20_500, 30_000, true)];
        assert_eq!(pick_window(&l, 0, 0, s(40_000), 0, now, &c), Some((0, s(20_000))));
        // refined_until 落後於 ring_start → 從 ring_start 起算
        let l = vec![led(1, 100_000, 104_000, true), led(2, 104_500, 109_000, true)];
        assert_eq!(pick_window(&l, 0, s(100_000), s(120_000), 0, now, &c), Some((s(100_000), s(109_000))));
    }
}
