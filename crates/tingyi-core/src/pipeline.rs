//! 管線:擷取 → VAD → ASR → MT,對外以事件串流回報。
//!
//! 執行緒配置:
//! - capture thread(WASAPI)→ crossbeam channel(AudioChunk)
//! - worker thread:VAD 切段 + SenseVoice 辨識(皆同步、CPU)→ tokio mpsc(Transcript)
//! - mt task(tokio):依序翻譯,處理短句合併與上下文
//!
//! 所有對外事件經 `tokio::sync::mpsc::UnboundedSender<PipelineEvent>`。

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 句子編號跨回合遞增:停止再開始時不會與上一輪的編號重複(前端以編號覆蓋同句)。
static NEXT_SENTENCE_ID: AtomicU64 = AtomicU64::new(1);
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::asr::{Lang, SenseVoice, SenseVoiceConfig, Transcriber};
use crate::audio::capture::{start_loopback, AudioChunk, CaptureConfig, CaptureHandle};
use crate::metrics::{SentenceTiming, TimingSnapshot};
use crate::mt::{TranslateRequest, Translator};
use crate::postprocess::{postprocess, Glossary};
use crate::vad::{is_fragment, Segment, Segmenter, VadConfig};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    Status { message: String },
    /// VAD 切出一段。
    Segment { id: u64, audio_ms: u32 },
    /// 辨識完成(可能因 is_fragment 被暫存,等下一段合併)。
    Transcript {
        id: u64,
        text: String,
        lang: Lang,
        asr_ms: u32,
        /// 判定為中文或不在允許語言 → 不翻譯。
        will_translate: bool,
    },
    TranslationDelta { id: u64, delta: String },
    TranslationDone {
        id: u64,
        source: String,
        text: String,
        timing: TimingSnapshot,
    },
    /// 翻譯失敗或逾時;前端以原文顯示。
    TranslationFailed { id: u64, error: String },
    Error { message: String },
}

#[derive(Clone)]
pub struct PipelineConfig {
    pub capture: CaptureConfig,
    pub vad: VadConfig,
    pub asr: SenseVoiceConfig,
    /// 允許的來源語言;空集合 = 全部允許。
    pub allowed_langs: HashSet<Lang>,
    /// 使用者鎖定語言(覆蓋自動偵測)。
    pub lock_lang: Option<Lang>,
    pub glossary: Glossary,
    pub context_sentences: usize,
    /// 短句等待合併的最長時間。
    pub merge_wait: Duration,
    /// 翻譯最長等待;超過視為失敗(前端顯示原文)。
    pub mt_timeout: Duration,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            capture: CaptureConfig::default(),
            vad: VadConfig::default(),
            asr: SenseVoiceConfig::default(),
            allowed_langs: [Lang::En, Lang::Ja, Lang::Ko].into_iter().collect(),
            lock_lang: None,
            glossary: Glossary::default(),
            context_sentences: 6,
            merge_wait: Duration::from_millis(1500),
            mt_timeout: Duration::from_secs(15),
        }
    }
}

/// 從 worker 送到 MT task 的一句。
struct Utterance {
    id: u64,
    text: String,
    lang: Lang,
    timing: SentenceTiming,
}

pub struct PipelineHandle {
    stop: Arc<AtomicBool>,
    capture: Option<CaptureHandle>,
    worker: Option<std::thread::JoinHandle<()>>,
    mt_task: Option<tokio::task::JoinHandle<()>>,
}

impl PipelineHandle {
    pub async fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.capture.take() {
            c.stop(); // 關掉擷取 → channel 斷 → worker 結束
        }
        if let Some(w) = self.worker.take() {
            let _ = tokio::task::spawn_blocking(move || w.join()).await;
        }
        if let Some(t) = self.mt_task.take() {
            let _ = t.await;
        }
    }
}

/// 啟動整條管線。`translator` 為 None 時只做辨識(bench asr 用)。
pub fn start(
    cfg: PipelineConfig,
    translator: Option<Arc<dyn Translator>>,
    events: mpsc::UnboundedSender<PipelineEvent>,
) -> Result<PipelineHandle> {
    let stop = Arc::new(AtomicBool::new(false));

    // 先載模型,失敗就直接回錯,不啟動擷取。
    let asr = SenseVoice::new(&cfg.asr)?;
    let segmenter = Segmenter::new(&cfg.vad)?;
    let _ = events.send(PipelineEvent::Status {
        message: format!("模型載入完成:{}", asr.name()),
    });

    let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioChunk>(256);
    let (utt_tx, utt_rx) = mpsc::unbounded_channel::<Utterance>();

    let capture = start_loopback(cfg.capture.clone(), audio_tx)?;

    let worker = {
        let events = events.clone();
        let stop = stop.clone();
        let cfg = cfg.clone();
        std::thread::Builder::new()
            .name("tingyi-worker".into())
            .spawn(move || worker_loop(cfg, segmenter, asr, audio_rx, utt_tx, events, stop))
            .map_err(|e| anyhow!("spawn worker: {e}"))?
    };

    let mt_task = {
        let events = events.clone();
        let cfg = cfg.clone();
        tokio::spawn(mt_loop(cfg, translator, utt_rx, events))
    };

    Ok(PipelineHandle {
        stop,
        capture: Some(capture),
        worker: Some(worker),
        mt_task: Some(mt_task),
    })
}

fn worker_loop(
    cfg: PipelineConfig,
    mut segmenter: Segmenter,
    asr: SenseVoice,
    audio_rx: crossbeam_channel::Receiver<AudioChunk>,
    utt_tx: mpsc::UnboundedSender<Utterance>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    stop: Arc<AtomicBool>,
) {
    let mut next_id: u64 = 0; // 未使用,保留簽名;實際編號取自 NEXT_SENTENCE_ID
    let mut last_lang: Option<Lang> = None;

    let handle_segment = |seg: Segment, _next_id: &mut u64, last_lang: &mut Option<Lang>| {
        let id = NEXT_SENTENCE_ID.fetch_add(1, Ordering::Relaxed);
        let audio_ms = seg.duration_ms();
        let _ = events.send(PipelineEvent::Segment { id, audio_ms });
        let mut timing = SentenceTiming::new(id, seg.end_at, audio_ms);

        match asr.transcribe(&seg.samples, cfg.lock_lang) {
            Ok(mut t) => {
                timing.asr_done = Some(Instant::now());
                if t.text.is_empty() {
                    return;
                }
                // 自動偵測信心低(Unknown)時沿用上一句語言。
                if t.lang == Lang::Unknown {
                    if let Some(l) = *last_lang {
                        t.lang = l;
                    }
                }
                if t.lang != Lang::Unknown {
                    *last_lang = Some(t.lang);
                }
                let will_translate = t.lang != Lang::Zh
                    && (cfg.allowed_langs.is_empty() || cfg.allowed_langs.contains(&t.lang));
                let _ = events.send(PipelineEvent::Transcript {
                    id,
                    text: t.text.clone(),
                    lang: t.lang,
                    asr_ms: t.asr_ms,
                    will_translate,
                });
                if will_translate {
                    let _ = utt_tx.send(Utterance {
                        id,
                        text: t.text,
                        lang: t.lang,
                        timing,
                    });
                }
            }
            Err(e) => {
                let _ = events.send(PipelineEvent::Error {
                    message: format!("辨識失敗:{e:#}"),
                });
            }
        }
    };

    while !stop.load(Ordering::SeqCst) {
        match audio_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                for seg in segmenter.push(&chunk.samples) {
                    handle_segment(seg, &mut next_id, &mut last_lang);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    for seg in segmenter.flush() {
        handle_segment(seg, &mut next_id, &mut last_lang);
    }
    tracing::info!("worker 結束");
}

async fn mt_loop(
    cfg: PipelineConfig,
    translator: Option<Arc<dyn Translator>>,
    mut rx: mpsc::UnboundedReceiver<Utterance>,
    events: mpsc::UnboundedSender<PipelineEvent>,
) {
    let Some(translator) = translator else {
        // 無翻譯器:把 utterance 吃掉即可。
        while rx.recv().await.is_some() {}
        return;
    };
    let mut context: VecDeque<(String, String)> = VecDeque::new();
    let mut pending: Option<Utterance> = None;

    loop {
        // 有暫存短句時,最多等 merge_wait 看有沒有下一段可合併。
        let next = if let Some(p) = &pending {
            let waited = p.timing.vad_end.elapsed();
            let remain = cfg.merge_wait.saturating_sub(waited);
            match tokio::time::timeout(remain, rx.recv()).await {
                Ok(Some(u)) => Some(u),
                Ok(None) => None,            // channel 關閉
                Err(_) => {
                    // 逾時:送出暫存句
                    let u = pending.take().unwrap();
                    translate_one(&cfg, &translator, &events, &mut context, u).await;
                    continue;
                }
            }
        } else {
            rx.recv().await
        };

        let Some(mut u) = next else {
            if let Some(p) = pending.take() {
                translate_one(&cfg, &translator, &events, &mut context, p).await;
            }
            break;
        };

        if let Some(p) = pending.take() {
            // 合併:保留較早句的 timing 原點(延遲算最壞情況)。
            let sep = if p.lang.is_cjk() { "" } else { " " };
            u.text = format!("{}{}{}", p.text, sep, u.text);
            u.timing.vad_end = p.timing.vad_end;
            u.timing.audio_ms += p.timing.audio_ms;
            u.id = p.id;
        }

        if is_fragment(&u.text, u.lang.is_cjk()) {
            pending = Some(u);
            continue;
        }
        translate_one(&cfg, &translator, &events, &mut context, u).await;
    }
    tracing::info!("mt task 結束");
}

async fn translate_one(
    cfg: &PipelineConfig,
    translator: &Arc<dyn Translator>,
    events: &mpsc::UnboundedSender<PipelineEvent>,
    context: &mut VecDeque<(String, String)>,
    mut u: Utterance,
) {
    let hits: Vec<_> = cfg.glossary.hits(&u.text).into_iter().cloned().collect();
    let req = TranslateRequest {
        text: u.text.clone(),
        src_lang: u.lang,
        context: context.iter().cloned().collect(),
        glossary: hits.clone(),
    };
    let id = u.id;
    let ev = events.clone();
    let mut first: Option<Instant> = None;
    let mut on_delta = move |d: &str| {
        if first.is_none() {
            first = Some(Instant::now());
        }
        let _ = ev.send(PipelineEvent::TranslationDelta {
            id,
            delta: d.to_string(),
        });
    };

    let fut = translator.translate(req, &mut on_delta);
    match tokio::time::timeout(cfg.mt_timeout, fut).await {
        Ok(Ok(res)) => {
            let now = Instant::now();
            u.timing.mt_done = Some(now);
            u.timing.mt_first_token = res
                .first_token_ms
                .and_then(|ms| u.timing.asr_done.map(|a| a + Duration::from_millis(ms as u64)))
                .or(Some(now));
            let hit_refs: Vec<_> = hits.iter().collect();
            let text = postprocess(&res.text, &hit_refs);
            context.push_back((u.text.clone(), text.clone()));
            while context.len() > cfg.context_sentences {
                context.pop_front();
            }
            let _ = events.send(PipelineEvent::TranslationDone {
                id,
                source: u.text,
                text,
                timing: u.timing.snapshot(),
            });
        }
        Ok(Err(e)) => {
            let _ = events.send(PipelineEvent::TranslationFailed {
                id,
                error: format!("{e:#}"),
            });
        }
        Err(_) => {
            let _ = events.send(PipelineEvent::TranslationFailed {
                id,
                error: format!("翻譯逾時(> {:?})", cfg.mt_timeout),
            });
        }
    }
}
