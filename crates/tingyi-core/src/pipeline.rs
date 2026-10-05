//! 管線:擷取 → VAD → ASR → 句子合併 → MT,對外以事件串流回報。
//!
//! 執行緒配置:
//! - capture thread(WASAPI)→ crossbeam channel(AudioChunk)
//! - worker thread:VAD 切段 + 音量正規化 + SenseVoice 辨識(皆同步、CPU)→ tokio mpsc(Utterance)
//! - mt task(tokio):依句子完整度決定等待/合併,再依序翻譯
//!
//! 所有對外事件經 `tokio::sync::mpsc::UnboundedSender<PipelineEvent>`。

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::asr::{Lang, SenseVoice, SenseVoiceConfig, Transcriber};
use crate::audio::capture::{start_loopback, AudioChunk, CaptureConfig, CaptureHandle};
use crate::audio::{normalize_rms, rms_dbfs, TARGET_SAMPLE_RATE};
use crate::metrics::{SentenceTiming, TimingSnapshot};
use crate::mt::{TranslateRequest, Translator};
use crate::postprocess::{postprocess, Glossary};
use crate::sentence::{completeness, Completeness};
use crate::vad::{Segment, Segmenter, VadConfig};

/// 句子編號跨回合遞增:停止再開始時不會與上一輪的編號重複(前端以編號覆蓋同句)。
static NEXT_SENTENCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    Status { message: String },
    /// VAD 切出一段。
    Segment {
        id: u64,
        audio_ms: u32,
        /// 原始段落音量(dBFS),診斷音量與辨識品質的關係。
        rms_dbfs: f32,
        /// 若有錄音,存檔路徑。
        wav: Option<String>,
    },
    /// 辨識完成(可能因句子未完被暫存,等下一段合併)。
    Transcript {
        id: u64,
        text: String,
        lang: Lang,
        asr_ms: u32,
        /// 判定為中文或不在允許語言 → 不翻譯。
        will_translate: bool,
    },
    /// `from` 那段被併入 `into`;`text` 為合併後原文。前端應移除 `from`、更新 `into`。
    TranscriptMerged { into: u64, from: u64, text: String },
    /// 講話中的即時預覽(暫定原文,不翻譯);空字串 = 清除。段落結束後由正式 `Transcript` 取代。
    Partial { text: String, lang: Lang },
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
    /// 前文採「累加式」:舊句固定不動、只在尾端加新句,讓 llama-server 的提示快取命中;
    /// 累計字元超過 `context_max_chars` 時整批清到剩最近 `context_sentences` 句(快取失效一次後重建)。
    pub context_sentences: usize,
    pub context_max_chars: usize,
    /// 句子明顯沒說完時,最多等下一段多久。
    pub incomplete_wait: Duration,
    /// 看不出完整與否時的短暫等待。
    pub unknown_wait: Duration,
    /// 合併後音訊長度上限,超過就強制翻。
    pub merge_max_audio_ms: u32,
    /// 合併後字數上限(CJK 字數;英文以詞數 × 2 近似)。
    pub merge_max_chars: usize,
    /// 翻譯最長等待;超過視為失敗(前端顯示原文)。
    pub mt_timeout: Duration,
    /// 送辨識前做音量正規化。
    pub normalize_audio: bool,
    /// 有設就把每段原始 16 kHz 樣本存成 wav(bench / 除錯用)。
    pub record_dir: Option<PathBuf>,
    /// 講話中每隔 `preview_interval` 對目前語音重辨識一次,以灰字先顯示原文。
    pub preview: bool,
    pub preview_interval: Duration,
    /// 語音累積到這麼長才開始預覽。
    pub preview_min: Duration,
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
            context_max_chars: 1600,
            incomplete_wait: Duration::from_millis(2500),
            unknown_wait: Duration::from_millis(500),
            merge_max_audio_ms: 12_000,
            merge_max_chars: 80,
            mt_timeout: Duration::from_secs(15),
            normalize_audio: true,
            record_dir: None,
            preview: true,
            preview_interval: Duration::from_millis(1000),
            preview_min: Duration::from_millis(1200),
        }
    }
}

/// 從 worker 送到 MT task 的一句。
struct Utterance {
    id: u64,
    text: String,
    lang: Lang,
    timing: SentenceTiming,
    /// 這句(含已合併的)到達 MT 階段的時間,用來算等待逾時。
    arrived: Instant,
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
    if let Some(d) = &cfg.record_dir {
        std::fs::create_dir_all(d).map_err(|e| anyhow!("建立錄音目錄 {}: {e}", d.display()))?;
    }
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
    let mut last_lang: Option<Lang> = None;

    let handle_segment = |seg: Segment, last_lang: &mut Option<Lang>| {
        let id = NEXT_SENTENCE_ID.fetch_add(1, Ordering::Relaxed);
        let audio_ms = seg.duration_ms();
        let level = rms_dbfs(&seg.samples);
        let wav = cfg.record_dir.as_ref().map(|d| {
            let p = d.join(format!("seg-{id:04}.wav"));
            if !sherpa_onnx::write(&p.to_string_lossy(), &seg.samples, TARGET_SAMPLE_RATE) {
                tracing::warn!("寫入 {} 失敗", p.display());
            }
            p.to_string_lossy().to_string()
        });
        let _ = events.send(PipelineEvent::Segment {
            id,
            audio_ms,
            rms_dbfs: level,
            wav,
        });
        let mut timing = SentenceTiming::new(id, seg.end_at, audio_ms);

        let pcm = if cfg.normalize_audio {
            normalize_rms(&seg.samples, -20.0, 8.0)
        } else {
            seg.samples.clone()
        };

        match asr.transcribe(&pcm, cfg.lock_lang) {
            Ok(mut t) => {
                timing.asr_done = Some(Instant::now());
                if t.text.is_empty() {
                    // 段落沒有文字(雜音):把可能殘留的預覽清掉
                    let _ = events.send(PipelineEvent::Partial {
                        text: String::new(),
                        lang: Lang::Unknown,
                    });
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
                        arrived: Instant::now(),
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

    let mut last_preview = Instant::now();
    let mut preview_shown = false;
    let min_preview_samples = (cfg.preview_min.as_millis() as usize) * TARGET_SAMPLE_RATE as usize / 1000;

    while !stop.load(Ordering::SeqCst) {
        match audio_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                let segs = segmenter.push(&chunk.samples);
                let had_segment = !segs.is_empty();
                for seg in segs {
                    handle_segment(seg, &mut last_lang);
                }
                if had_segment && preview_shown {
                    // 正式 Transcript 事件會直接取代預覽(前端在 transcript 時清空 partial),
                    // 這裡不另送清空事件,避免「清空 → 補上」的閃動。
                    preview_shown = false;
                }
                if cfg.preview && !had_segment && last_preview.elapsed() >= cfg.preview_interval {
                    if let Some(cur) = segmenter.current_speech() {
                        if cur.len() >= min_preview_samples {
                            last_preview = Instant::now();
                            let pcm = if cfg.normalize_audio {
                                normalize_rms(&cur, -20.0, 8.0)
                            } else {
                                cur
                            };
                            if let Ok(t) = asr.transcribe(&pcm, cfg.lock_lang.or(last_lang)) {
                                if !t.text.is_empty() {
                                    preview_shown = true;
                                    let _ = events.send(PipelineEvent::Partial {
                                        text: t.text,
                                        lang: t.lang,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    for seg in segmenter.flush() {
        handle_segment(seg, &mut last_lang);
    }
    tracing::info!("worker 結束");
}

/// 合併後是否已達硬上限,必須翻。
fn merge_limit_reached(cfg: &PipelineConfig, u: &Utterance) -> bool {
    if u.timing.audio_ms >= cfg.merge_max_audio_ms {
        return true;
    }
    let len = if u.lang.is_cjk() {
        u.text.chars().count()
    } else {
        u.text.split_whitespace().count() * 2
    };
    len >= cfg.merge_max_chars
}

/// 這句該等多久:None = 立刻翻。
fn wait_for(cfg: &PipelineConfig, u: &Utterance) -> Option<Duration> {
    if merge_limit_reached(cfg, u) {
        return None;
    }
    match completeness(&u.text, u.lang) {
        Completeness::Complete => None,
        Completeness::Incomplete => Some(cfg.incomplete_wait),
        Completeness::Unknown => Some(cfg.unknown_wait),
    }
}

fn merge(cfg: &PipelineConfig, mut p: Utterance, u: Utterance, events: &mpsc::UnboundedSender<PipelineEvent>) -> Utterance {
    let sep = if p.lang.is_cjk() && u.lang.is_cjk() { "" } else { " " };
    p.text = format!("{}{}{}", p.text, sep, u.text);
    // 保留較早句的 timing 原點(延遲算最壞情況),音訊長度累加。
    p.timing.audio_ms += u.timing.audio_ms;
    if let (Some(a), Some(b)) = (p.timing.asr_done, u.timing.asr_done) {
        p.timing.asr_done = Some(a.max(b));
    }
    p.arrived = u.arrived;
    if p.lang == Lang::Unknown {
        p.lang = u.lang;
    }
    let _ = events.send(PipelineEvent::TranscriptMerged {
        into: p.id,
        from: u.id,
        text: p.text.clone(),
    });
    let _ = cfg;
    p
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
        let next = if let Some(p) = &pending {
            // 暫存句等待中:依完整度決定最多等多久(從它到達 MT 階段算起)。
            let budget = wait_for(&cfg, p).unwrap_or(Duration::ZERO);
            let remain = budget.saturating_sub(p.arrived.elapsed());
            if remain.is_zero() {
                let u = pending.take().unwrap();
                translate_one(&cfg, &translator, &events, &mut context, u).await;
                continue;
            }
            match tokio::time::timeout(remain, rx.recv()).await {
                Ok(Some(u)) => Some(u),
                Ok(None) => None, // channel 關閉
                Err(_) => {
                    let u = pending.take().unwrap();
                    translate_one(&cfg, &translator, &events, &mut context, u).await;
                    continue;
                }
            }
        } else {
            rx.recv().await
        };

        let Some(u) = next else {
            if let Some(p) = pending.take() {
                translate_one(&cfg, &translator, &events, &mut context, p).await;
            }
            break;
        };

        let u = match pending.take() {
            Some(p) => merge(&cfg, p, u, &events),
            None => u,
        };

        // 合併後重新判斷:還沒說完就繼續等,說完或達上限就翻。
        if wait_for(&cfg, &u).is_some() {
            pending = Some(u);
        } else {
            translate_one(&cfg, &translator, &events, &mut context, u).await;
        }
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
    let mut on_delta = move |d: &str| {
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
                .map(|ms| now - Duration::from_millis(res.total_ms.saturating_sub(ms) as u64))
                .or(Some(now));
            let hit_refs: Vec<_> = hits.iter().collect();
            let text = postprocess(&res.text, &hit_refs);
            context.push_back((u.text.clone(), text.clone()));
            // 累加式前文:平時只追加(提示前綴不變 → 快取命中);超過額度才一次裁到最近幾句。
            let total: usize = context.iter().map(|(s, t)| s.chars().count() + t.chars().count()).sum();
            if total > cfg.context_max_chars {
                while context.len() > cfg.context_sentences {
                    context.pop_front();
                }
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
