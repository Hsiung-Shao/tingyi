//! 管線:擷取 → VAD(+預覽)→ 正式辨識(+說話者)→ 句子合併 → MT,對外以事件串流回報。
//!
//! 執行緒配置:
//! - capture thread(WASAPI)→ crossbeam channel(AudioChunk)
//! - vad thread:VAD 切段、即時預覽、套用斷句參數 → crossbeam channel(Segment)
//! - asr thread:音量正規化、正式辨識、說話者、斷句觀察值回饋 → tokio mpsc(Utterance)
//!   (辨識與說話者搬離 vad thread,Whisper 的 150–200 ms 不會卡住 VAD 與預覽)
//! - mt task(tokio):依句子完整度與「講者是否已停」決定等待/合併,再依序翻譯
//!
//! 所有對外事件經 `tokio::sync::mpsc::UnboundedSender<PipelineEvent>`。

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::asr::{Lang, SenseVoice, SenseVoiceConfig, Transcriber, WhisperServer, WhisperServerConfig};
use crate::cleanup::{clean_transcript, Replacement};
use crate::route::{resolve_load_set, RoutedTranscriber};
use crate::audio::capture::{start_capture, AudioChunk, CaptureConfig, CaptureHandle};
use crate::audio::source::SourceControl;
use crate::audio::ring::samples_to_ms;
use crate::audio::wav_sink::WavSink;
use crate::audio::{normalize_rms, rms_dbfs, TARGET_SAMPLE_RATE};
use crate::refine::{RefineConfig, RefineJob, RefineMsg};
use crate::metrics::{PipelineStats, SentenceTiming, TimingSnapshot};
use crate::mt::{TranslateRequest, Translator};
use crate::postprocess::{postprocess, Glossary};
use crate::sentence::{completeness, Completeness};
use crate::speaker::{SpeakerConfig, SpeakerIdentifier};
use crate::tuner::{SegControl, SegStat, SegmentationTuner};
use crate::vad::{Segment, Segmenter, VadConfig};

/// 說話者辨識器的共享句柄(Tauri 改名命令用)。
pub type SharedSpeakers = Arc<Mutex<Option<SpeakerIdentifier>>>;

/// 句子編號跨回合遞增:停止再開始時不會與上一輪的編號重複(前端以編號覆蓋同句)。
pub(crate) static NEXT_SENTENCE_ID: AtomicU64 = AtomicU64::new(1);

/// 段落正式辨識用的引擎;即時預覽一律用 SenseVoice。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrEngine {
    SenseVoice,
    /// 需要已啟動的 whisper-server(見 `llama::WhisperServerProcess`)。
    Whisper { base_url: String },
    /// 依語言路由專用模型(CPU):日文 ReazonSpeech、英文 Parakeet,其餘 SenseVoice。
    Routed,
}

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
        /// 說話者標籤(開啟說話者辨識且段落夠長時才有)。
        speaker: Option<String>,
        /// 這段在整條串流中的位置(毫秒,原點 = 擷取開始)。
        start_ms: u64,
        end_ms: u64,
    },
    /// 斷句參數變更(自適應或使用者調整)。
    Segmentation {
        min_silence_ms: u32,
        max_speech_ms: u32,
        reason: String,
    },
    /// 已知說話者清單(新增或改名時重送)。
    Speakers { names: Vec<String> },
    /// 系統自動把 `from` 併入 `into`(聲紋太像);前端把舊標籤換成新標籤。
    SpeakerMerged { from: String, into: String },
    /// `from` 那段被併入 `into`;`text` 為合併後原文。前端應移除 `from`、更新 `into`。
    TranscriptMerged { into: u64, from: u64, text: String, start_ms: u64, end_ms: u64 },
    /// 回修:用更長的連續音訊重新辨識後,這句原文有變。
    SourceRevised { id: u64, text: String },
    /// 回修:帶前後文重翻後的譯文,取代快譯。
    TranslationRevised { id: u64, text: String },
    /// 回修:VAD 漏掉的語音被補回成新句;插在 `after` 之後(None = 視窗第一句之前)。
    Recovered {
        id: u64,
        after: Option<u64>,
        text: String,
        lang: Lang,
        speaker: Option<String>,
        start_ms: u64,
        end_ms: u64,
        will_translate: bool,
    },
    /// 講話中的即時預覽(暫定原文,不翻譯);空字串 = 清除。段落結束後由正式 `Transcript` 取代。
    /// `speaker` 為只比對不學習的暫定說話者。
    Partial { text: String, lang: Lang, speaker: Option<String> },
    TranslationDelta { id: u64, delta: String },
    TranslationDone {
        id: u64,
        source: String,
        text: String,
        timing: TimingSnapshot,
        speaker: Option<String>,
    },
    /// 翻譯失敗或逾時;前端以原文顯示。
    TranslationFailed { id: u64, error: String },
    Error { message: String },
}

#[derive(Clone)]
pub struct PipelineConfig {
    pub capture: CaptureConfig,
    /// 音訊來源(系統或指定程式);Tauri 持有同一個 Arc,執行中切換即時生效。
    pub audio_source: Arc<SourceControl>,
    pub vad: VadConfig,
    pub asr: SenseVoiceConfig,
    pub asr_engine: AsrEngine,
    /// 允許的來源語言;空集合 = 全部允許。
    pub allowed_langs: HashSet<Lang>,
    /// 使用者鎖定語言(覆蓋自動偵測)。
    pub lock_lang: Option<Lang>,
    pub glossary: Glossary,
    /// 辨識取代表(錯字 → 正字),四個出口(正式、預覽、回修改原文、回修補回句)都套。
    pub replacements: Vec<Replacement>,
    /// 幻聽式重複壓縮 + 取代表;bench A/B 可關。
    pub cleanup: bool,
    /// 短稿補救(字數明顯太少時切兩半重解);bench A/B 後決定預設。
    pub rescue: bool,
    /// 前文採「累加式」:舊句固定不動、只在尾端加新句,讓 llama-server 的提示快取命中;
    /// 累計字元超過 `context_max_chars` 時整批清到剩最近 `context_sentences` 句(快取失效一次後重建)。
    pub context_sentences: usize,
    pub context_max_chars: usize,
    /// 句子明顯沒說完時,最多等下一段多久。
    pub incomplete_wait: Duration,
    /// 看不出完整與否時的最長等待。
    pub unknown_wait: Duration,
    /// 講者已連續靜音這麼久 → 看不出完整與否的句子直接翻(不再等)。
    pub unknown_silence: Duration,
    /// 講者已連續靜音這麼久 → 明顯未完的句子也直接翻(對方大概不會接著講了)。
    pub incomplete_silence: Duration,
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
    /// 斷句控制面(偏移、自動開關、目前值);Tauri 持有同一個 Arc 即可即時調整。
    pub seg_control: Arc<SegControl>,
    /// 依語言預設與語速調整 VAD 參數;關掉則全程用 `vad` 的設定(bench 固定參數比較用)。
    pub seg_tuning: bool,
    /// 說話者辨識(模型存在時才生效)。
    pub speaker_id: bool,
    pub speaker: SpeakerConfig,
    /// 回修(慢路徑)。
    pub refine: RefineConfig,
    /// 有設就把整條連續 16 kHz 音訊寫成一個 wav(bench 離線對照用)。
    pub record_all: Option<PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            capture: CaptureConfig::default(),
            audio_source: Arc::new(SourceControl::default()),
            vad: VadConfig::default(),
            asr: SenseVoiceConfig::default(),
            asr_engine: AsrEngine::SenseVoice,
            allowed_langs: [Lang::En, Lang::Ja, Lang::Ko].into_iter().collect(),
            lock_lang: None,
            glossary: Glossary::default(),
            replacements: Vec::new(),
            cleanup: true,
            rescue: false,
            context_sentences: 6,
            context_max_chars: 1600,
            incomplete_wait: Duration::from_millis(2500),
            unknown_wait: Duration::from_millis(500),
            unknown_silence: Duration::from_millis(250),
            // 2026-10-06 使用者決定:有回修補償後快路徑更積極,未完句靜音 0.6 s 就翻(原 1.2 s)
            incomplete_silence: Duration::from_millis(600),
            merge_max_audio_ms: 12_000,
            merge_max_chars: 80,
            mt_timeout: Duration::from_secs(15),
            normalize_audio: true,
            record_dir: None,
            preview: true,
            preview_interval: Duration::from_millis(1000),
            preview_min: Duration::from_millis(1200),
            seg_control: Arc::new(SegControl::default()),
            seg_tuning: true,
            speaker_id: true,
            speaker: SpeakerConfig::default(),
            refine: RefineConfig::default(),
            record_all: None,
        }
    }
}

/// 從 asr thread 送到 MT task 的一句。
struct Utterance {
    id: u64,
    text: String,
    lang: Lang,
    speaker: Option<String>,
    /// 在整條串流中的樣本區間(回修對齊用)。
    start_sample: i64,
    end_sample: i64,
    timing: SentenceTiming,
    /// 這句(含已合併的)到達 MT 階段的時間,用來算等待逾時。
    arrived: Instant,
}

/// asr thread 回饋給 vad thread 的斷句觀察值。
struct SegObservation {
    lang: Lang,
    stat: SegStat,
}

pub struct PipelineHandle {
    stop: Arc<AtomicBool>,
    capture: Option<CaptureHandle>,
    vad_thread: Option<std::thread::JoinHandle<()>>,
    asr_thread: Option<std::thread::JoinHandle<()>>,
    mt_task: Option<tokio::task::JoinHandle<()>>,
    refine_thread: Option<std::thread::JoinHandle<()>>,
    refine_mt_task: Option<tokio::task::JoinHandle<()>>,
    speakers: SharedSpeakers,
    events: mpsc::UnboundedSender<PipelineEvent>,
    stats: Arc<PipelineStats>,
}

impl PipelineHandle {
    /// 效能計數(前端每 2 s 讀一次)。
    pub fn stats(&self) -> Arc<PipelineStats> {
        self.stats.clone()
    }

    /// 說話者改名;成功則重送 `Speakers` 事件。
    pub fn rename_speaker(&self, from: &str, to: &str) -> bool {
        let mut guard = self.speakers.lock().unwrap();
        let Some(id) = guard.as_mut() else { return false };
        let ok = id.rename(from, to);
        if ok {
            let _ = self.events.send(PipelineEvent::Speakers { names: id.speakers() });
        }
        ok
    }

    pub fn speakers(&self) -> Vec<String> {
        self.speakers
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.speakers())
            .unwrap_or_default()
    }

    pub async fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.join_all().await;
    }

    /// 等已送入的音訊全部處理完再結束(不設 stop,不截斷尾巴)。
    /// `start_with_audio` 的呼叫端要先丟掉送音訊的 sender。
    pub async fn finish(mut self) {
        self.join_all().await;
    }

    async fn join_all(&mut self) {
        if let Some(c) = self.capture.take() {
            c.stop(); // 關掉擷取 → channel 斷 → vad thread 結束 → seg channel 斷 → asr thread 結束
        }
        if let Some(w) = self.vad_thread.take() {
            let _ = tokio::task::spawn_blocking(move || w.join()).await;
        }
        if let Some(w) = self.asr_thread.take() {
            let _ = tokio::task::spawn_blocking(move || w.join()).await;
        }
        if let Some(t) = self.mt_task.take() {
            let _ = t.await;
        }
        // refine thread 看到 stop 旗標(或 channel 斷線)結束並 drop job_tx → refine_mt 自然結束
        if let Some(w) = self.refine_thread.take() {
            let _ = tokio::task::spawn_blocking(move || w.join()).await;
        }
        if let Some(t) = self.refine_mt_task.take() {
            let _ = t.await;
        }
    }
}

/// 管線的音訊來源:即時擷取,或由呼叫端餵(bench 串流評測)。
enum AudioInput {
    Capture,
    Channel(crossbeam_channel::Receiver<AudioChunk>),
}

/// 啟動整條管線。`translator` 為 None 時只做辨識(bench asr 用)。
pub fn start(
    cfg: PipelineConfig,
    translator: Option<Arc<dyn Translator>>,
    events: mpsc::UnboundedSender<PipelineEvent>,
) -> Result<PipelineHandle> {
    start_inner(cfg, translator, events, AudioInput::Capture)
}

/// 不開擷取,改用呼叫端餵的 16 kHz 音訊跑整條管線(串流評測)。送完丟掉 sender,再呼叫 `finish()`。
pub fn start_with_audio(
    cfg: PipelineConfig,
    translator: Option<Arc<dyn Translator>>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    audio_rx: crossbeam_channel::Receiver<AudioChunk>,
) -> Result<PipelineHandle> {
    start_inner(cfg, translator, events, AudioInput::Channel(audio_rx))
}

fn start_inner(
    cfg: PipelineConfig,
    translator: Option<Arc<dyn Translator>>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    input: AudioInput,
) -> Result<PipelineHandle> {
    let stop = Arc::new(AtomicBool::new(false));

    // 先載模型,失敗就直接回錯,不啟動擷取。預覽永遠用 SenseVoice;正式辨識依設定。
    // 鎖定語言要在建立 SenseVoice 時就設好(之後每次呼叫傳的語言提示不會生效)
    let sv_cfg = SenseVoiceConfig { language: cfg.lock_lang, ..cfg.asr.clone() };
    let preview_asr = SenseVoice::new(&sv_cfg)?;
    let final_asr: Box<dyn Transcriber> = match &cfg.asr_engine {
        AsrEngine::SenseVoice => Box::new(SenseVoice::new(&sv_cfg)?),
        AsrEngine::Whisper { base_url } => Box::new(WhisperServer::new(WhisperServerConfig {
            base_url: base_url.clone(),
            language: cfg.lock_lang,
            ..Default::default()
        })),
        AsrEngine::Routed => Box::new(RoutedTranscriber::new(
            &sv_cfg,
            &resolve_load_set(cfg.lock_lang, &cfg.allowed_langs),
            crate::paths::REAZONSPEECH_JA_DIR_NAME,
        )?),
    };
    let segmenter = Segmenter::new(&cfg.vad)?;
    if let Some(d) = &cfg.record_dir {
        std::fs::create_dir_all(d).map_err(|e| anyhow!("建立錄音目錄 {}: {e}", d.display()))?;
    }
    let speakers: SharedSpeakers = Arc::new(Mutex::new(None));
    if cfg.speaker_id && cfg.speaker.model.exists() {
        match SpeakerIdentifier::new(cfg.speaker.clone()) {
            Ok(s) => *speakers.lock().unwrap() = Some(s),
            Err(e) => {
                let _ = events.send(PipelineEvent::Error {
                    message: format!("說話者模型載入失敗,停用:{e:#}"),
                });
            }
        }
    }
    let _ = events.send(PipelineEvent::Status {
        message: format!(
            "模型載入完成:{}(預覽 {}){}",
            final_asr.name(),
            preview_asr.name(),
            if speakers.lock().unwrap().is_some() { "、說話者辨識" } else { "" }
        ),
    });

    let (seg_tx, seg_rx) = crossbeam_channel::bounded::<Segment>(64);
    let (obs_tx, obs_rx) = crossbeam_channel::unbounded::<SegObservation>();
    let (utt_tx, utt_rx) = mpsc::unbounded_channel::<Utterance>();
    let stats = Arc::new(PipelineStats::default());

    // 回修:只有 Whisper 引擎有時間戳
    let refine_on = match (&cfg.refine.enabled, &cfg.asr_engine) {
        (true, AsrEngine::Whisper { .. }) => true,
        (true, AsrEngine::SenseVoice | AsrEngine::Routed) => {
            let _ = events.send(PipelineEvent::Status { message: "回修僅支援 Whisper 引擎,已停用".into() });
            false
        }
        _ => false,
    };
    let (refine_tx, refine_rx) = crossbeam_channel::unbounded::<RefineMsg>();
    let refine_tx: Option<crossbeam_channel::Sender<RefineMsg>> = if refine_on { Some(refine_tx) } else { None };
    let wav_sink = match &cfg.record_all {
        Some(p) => Some(WavSink::create(p)?),
        None => None,
    };

    let (capture, audio_rx) = match input {
        AudioInput::Capture => {
            let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioChunk>(256);
            let c = start_capture(cfg.capture.clone(), cfg.audio_source.clone(), stats.capture.clone(), audio_tx)?;
            (Some(c), audio_rx)
        }
        AudioInput::Channel(rx) => (None, rx),
    };

    let vad_thread = {
        let events = events.clone();
        let stop = stop.clone();
        let cfg = cfg.clone();
        let speakers = speakers.clone();
        let stats = stats.clone();
        let refine_tx = refine_tx.clone();
        std::thread::Builder::new()
            .name("tingyi-vad".into())
            .spawn(move || vad_loop(cfg, segmenter, preview_asr, speakers, stats, audio_rx, seg_tx, obs_rx, events, stop, refine_tx, wav_sink))
            .map_err(|e| anyhow!("spawn vad thread: {e}"))?
    };

    let asr_thread = {
        let events = events.clone();
        let cfg = cfg.clone();
        let speakers = speakers.clone();
        let stats = stats.clone();
        let refine_tx = refine_tx.clone();
        std::thread::Builder::new()
            .name("tingyi-asr".into())
            .spawn(move || asr_loop(cfg, final_asr, speakers, stats, seg_rx, obs_tx, utt_tx, events, refine_tx))
            .map_err(|e| anyhow!("spawn asr thread: {e}"))?
    };

    let mt_task = {
        let events = events.clone();
        let cfg = cfg.clone();
        let stats = stats.clone();
        let refine_tx = refine_tx.clone();
        tokio::spawn(mt_loop(cfg, translator.clone(), utt_rx, events, stats, refine_tx))
    };

    let (refine_thread, refine_mt_task) = if refine_on {
        let whisper_url = match (&cfg.refine.whisper_url, &cfg.asr_engine) {
            (Some(u), _) => u.clone(),
            (None, AsrEngine::Whisper { base_url }) => base_url.clone(),
            (None, AsrEngine::SenseVoice | AsrEngine::Routed) => unreachable!(),
        };
        let refine_asr = WhisperServer::new(WhisperServerConfig {
            base_url: whisper_url,
            language: cfg.lock_lang,
            timeout: Duration::from_secs(30),
        });
        let (job_tx, job_rx) = mpsc::unbounded_channel::<RefineJob>();
        let (job_tx, refine_mt_task) = match &translator {
            Some(tr) => {
                let events = events.clone();
                let cfg2 = cfg.clone();
                let stats2 = stats.clone();
                let ledger_tx = refine_tx.clone().expect("refine_tx");
                (Some(job_tx), Some(tokio::spawn(crate::refine::refine_mt_loop(cfg2, tr.clone(), job_rx, ledger_tx, events, stats2))))
            }
            None => (None, None),
        };
        let events = events.clone();
        let stop = stop.clone();
        let cfg2 = cfg.clone();
        let speakers = speakers.clone();
        let stats2 = stats.clone();
        let th = std::thread::Builder::new()
            .name("tingyi-refine".into())
            .spawn(move || crate::refine::refine_loop(cfg2, refine_asr, speakers, stats2, refine_rx, job_tx, events, stop))
            .map_err(|e| anyhow!("spawn refine thread: {e}"))?;
        (Some(th), refine_mt_task)
    } else {
        drop(refine_rx);
        (None, None)
    };

    Ok(PipelineHandle {
        stop,
        capture,
        vad_thread: Some(vad_thread),
        asr_thread: Some(asr_thread),
        mt_task: Some(mt_task),
        refine_thread,
        refine_mt_task,
        speakers,
        events,
        stats,
    })
}

/// 切換音訊來源時插入的靜音:比斷句靜音上限(600 ms)長,讓進行中的語音在切換點收尾。
const SWITCH_SILENCE_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 7 / 10;

/// 同一批樣本餵給回修環形緩衝、整段錄音與 Segmenter(三者時鐘一致)。
/// 回傳是否切出段落;下游已關閉回 None。
fn feed_samples(
    samples: &[f32],
    segmenter: &mut Segmenter,
    seg_tx: &crossbeam_channel::Sender<Segment>,
    refine_tx: &Option<crossbeam_channel::Sender<RefineMsg>>,
    wav_sink: &mut Option<WavSink>,
    stats: &PipelineStats,
) -> Option<bool> {
    if let Some(tx) = refine_tx {
        let _ = tx.send(RefineMsg::Audio(samples.to_vec()));
    }
    if let Some(w) = wav_sink.as_mut() {
        if let Err(e) = w.write(samples) {
            tracing::warn!("整段錄音寫入失敗:{e:#}");
            *wav_sink = None;
        }
    }
    let segs = segmenter.push(samples);
    stats.set_speaking(segmenter.speaking());
    let had_segment = !segs.is_empty();
    for seg in segs {
        seg_tx.send(seg).ok()?;
    }
    Some(had_segment)
}

fn send_segmentation(events: &mpsc::UnboundedSender<PipelineEvent>, segmenter: &mut Segmenter, p: crate::tuner::SegParams, enabled: bool) {
    if !enabled {
        return;
    }
    segmenter.set_params(p.min_silence_ms as f32 / 1000.0, p.max_speech_ms as f32 / 1000.0);
    let _ = events.send(PipelineEvent::Segmentation {
        min_silence_ms: p.min_silence_ms,
        max_speech_ms: p.max_speech_ms,
        reason: p.reason,
    });
}

/// VAD + 即時預覽 + 斷句參數套用。不做正式辨識,所以 Whisper / 說話者的耗時不會擋住切段。
#[allow(clippy::too_many_arguments)]
fn vad_loop(
    cfg: PipelineConfig,
    mut segmenter: Segmenter,
    preview_asr: SenseVoice,
    speakers: SharedSpeakers,
    stats: Arc<PipelineStats>,
    audio_rx: crossbeam_channel::Receiver<AudioChunk>,
    seg_tx: crossbeam_channel::Sender<Segment>,
    obs_rx: crossbeam_channel::Receiver<SegObservation>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    stop: Arc<AtomicBool>,
    refine_tx: Option<crossbeam_channel::Sender<RefineMsg>>,
    mut wav_sink: Option<WavSink>,
) {
    let mut tuner = SegmentationTuner::new();
    let mut last_lang: Option<Lang> = cfg.lock_lang;
    if let Some(l) = cfg.lock_lang {
        if let Some(p) = tuner.set_lang(l, &cfg.seg_control) {
            send_segmentation(&events, &mut segmenter, p, cfg.seg_tuning);
        }
    }
    let mut last_preview = Instant::now();
    let mut preview_shown = false;
    let mut cur_gen: Option<u64> = None;
    let min_preview_samples = (cfg.preview_min.as_millis() as usize) * TARGET_SAMPLE_RATE as usize / 1000;

    while !stop.load(Ordering::SeqCst) {
        match audio_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                // asr thread 回饋的觀察值 → tuner
                while let Ok(o) = obs_rx.try_recv() {
                    last_lang = Some(o.lang);
                    if let Some(p) = tuner.set_lang(o.lang, &cfg.seg_control) {
                        send_segmentation(&events, &mut segmenter, p, cfg.seg_tuning);
                    }
                    if let Some(p) = tuner.observe(o.stat, &cfg.seg_control) {
                        send_segmentation(&events, &mut segmenter, p, cfg.seg_tuning);
                    }
                }
                // 使用者改了偏移/自動開關 → 立刻重算參數
                if let Some(p) = tuner.emit_if_changed(&cfg.seg_control) {
                    send_segmentation(&events, &mut segmenter, p, cfg.seg_tuning);
                }

                stats.set_backlog(audio_rx.len());
                let mut had_segment = false;
                if cur_gen.is_some_and(|g| g != chunk.gen) {
                    // 切換來源:先插一段靜音讓進行中的語音收尾,再告訴回修不要跨過切換點
                    let zeros = vec![0.0f32; SWITCH_SILENCE_SAMPLES];
                    let Some(h) = feed_samples(&zeros, &mut segmenter, &seg_tx, &refine_tx, &mut wav_sink, &stats) else { return };
                    had_segment |= h;
                    if let Some(tx) = &refine_tx {
                        let _ = tx.send(RefineMsg::Boundary);
                    }
                    let _ = events.send(PipelineEvent::Status { message: "音訊來源已切換".into() });
                }
                cur_gen = Some(chunk.gen);
                let Some(h) = feed_samples(&chunk.samples, &mut segmenter, &seg_tx, &refine_tx, &mut wav_sink, &stats) else { return };
                had_segment |= h;
                if had_segment && preview_shown {
                    // 正式 Transcript 事件會直接取代預覽,不另送清空事件(避免閃動)
                    preview_shown = false;
                }

                // 減負:預覽平均超過 150 ms → 間隔拉到 1.5 倍;音訊積壓 > 5 chunk → 暫停預覽
                let preview_avg = {
                    let n = stats.preview_count.load(Ordering::Relaxed);
                    if n == 0 { 0 } else { stats.preview_total_ms.load(Ordering::Relaxed) / n }
                };
                let interval = if preview_avg > 150 { cfg.preview_interval.mul_f32(1.5) } else { cfg.preview_interval };
                let backlog_ok = audio_rx.len() <= 5;
                if cfg.preview && !had_segment && backlog_ok && last_preview.elapsed() >= interval {
                    if let Some(cur) = segmenter.current_speech() {
                        if cur.len() >= min_preview_samples {
                            last_preview = Instant::now();
                            let pv_t0 = Instant::now();
                            let pcm = if cfg.normalize_audio { normalize_rms(&cur, -20.0, 8.0) } else { cur };
                            if let Ok(t) = preview_asr.transcribe(&pcm, cfg.lock_lang.or(last_lang)) {
                                let text = clean(&cfg, t.text).unwrap_or_default();
                                if !text.is_empty() {
                                    preview_shown = true;
                                    // 先比對說話者(不學習),讓名字和灰字一起最早出現
                                    let speaker = speakers.lock().unwrap().as_ref().and_then(|s| s.peek(&pcm));
                                    let _ = events.send(PipelineEvent::Partial { text, lang: t.lang, speaker });
                                }
                            }
                            stats.record_preview(pv_t0.elapsed().as_millis() as u32);
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    for seg in segmenter.flush() {
        let _ = seg_tx.send(seg);
    }
    if let Some(w) = wav_sink.take() {
        if let Err(e) = w.finish() {
            tracing::warn!("整段錄音收尾失敗:{e:#}");
        }
    }
    tracing::info!("vad thread 結束");
}

/// 四個出口共用的清理;關閉時原樣回傳。整段是幻聽回 None。
pub(crate) fn clean(cfg: &PipelineConfig, text: String) -> Option<String> {
    if cfg.cleanup {
        clean_transcript(&text, &cfg.replacements)
    } else {
        Some(text)
    }
}

/// 短稿補救:字數明顯太少時在段內低能量處切兩半各自重解(兩側墊 0.35 s 合成靜音、語言固定),
/// 重解更長、字速正常且保留原結果尾段才採用。
fn rescue_text(asr: &dyn Transcriber, pcm: &[f32], text: &str, speech_ms: u32, lang: Lang, cfg: &PipelineConfig) -> Option<String> {
    if !crate::rescue::should_rescue(text, speech_ms, lang) {
        return None;
    }
    let at = crate::rescue::find_split(pcm, TARGET_SAMPLE_RATE as u32)?;
    let pad = vec![0.0f32; TARGET_SAMPLE_RATE as usize * 35 / 100];
    let mut parts = Vec::new();
    for half in [&pcm[..at], &pcm[at..]] {
        let mut padded = pad.clone();
        padded.extend_from_slice(half);
        padded.extend_from_slice(&pad);
        let t = asr.transcribe(&padded, Some(lang)).ok()?;
        if let Some(s) = clean(cfg, t.text).filter(|s| !s.is_empty()) {
            parts.push(s);
        }
    }
    let retry = parts.join(if lang.is_cjk() { "" } else { " " });
    let accepted = crate::rescue::accept_retry(text, &retry, speech_ms, lang);
    tracing::debug!("短稿補救({}):{text} → {retry}", if accepted { "採用" } else { "放棄" });
    accepted.then_some(retry)
}

/// 正式辨識 + 說話者 + 斷句觀察值回饋。
#[allow(clippy::too_many_arguments)]
fn asr_loop(
    cfg: PipelineConfig,
    asr: Box<dyn Transcriber>,
    speakers: SharedSpeakers,
    stats: Arc<PipelineStats>,
    seg_rx: crossbeam_channel::Receiver<Segment>,
    obs_tx: crossbeam_channel::Sender<SegObservation>,
    utt_tx: mpsc::UnboundedSender<Utterance>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    refine_tx: Option<crossbeam_channel::Sender<RefineMsg>>,
) {
    let mut last_lang: Option<Lang> = None;
    let mut known_speakers: usize = 0;

    while let Ok(seg) = seg_rx.recv() {
        let id = NEXT_SENTENCE_ID.fetch_add(1, Ordering::Relaxed);
        let audio_ms = seg.duration_ms();
        let start_sample = seg.start_sample;
        let end_sample = seg.start_sample + seg.samples.len() as i64;
        let level = rms_dbfs(&seg.samples);
        let wav = cfg.record_dir.as_ref().map(|d| {
            let p = d.join(format!("seg-{id:04}.wav"));
            if !sherpa_onnx::write(&p.to_string_lossy(), &seg.samples, TARGET_SAMPLE_RATE) {
                tracing::warn!("寫入 {} 失敗", p.display());
            }
            p.to_string_lossy().to_string()
        });
        let _ = events.send(PipelineEvent::Segment { id, audio_ms, rms_dbfs: level, wav });
        let mut timing = SentenceTiming::new(id, seg.end_at, audio_ms);

        let pcm = if cfg.normalize_audio { normalize_rms(&seg.samples, -20.0, 8.0) } else { seg.samples.clone() };
        let core_ms = seg.core_ms();
        let hit_max = seg.forced;

        let mut t = match asr.transcribe(&pcm, cfg.lock_lang) {
            Ok(t) => t,
            Err(e) => {
                let _ = events.send(PipelineEvent::Error { message: format!("辨識失敗:{e:#}") });
                continue;
            }
        };
        match clean(&cfg, std::mem::take(&mut t.text)) {
            Some(text) => t.text = text,
            None => tracing::debug!("丟棄幻聽段落 #{id}"),
        }
        if cfg.rescue && !t.text.is_empty() {
            let lang = cfg.lock_lang.unwrap_or(t.lang);
            if let Some(text) = rescue_text(asr.as_ref(), &pcm, &t.text, core_ms, lang, &cfg) {
                t.text = text;
            }
        }
        timing.asr_done = Some(Instant::now());
        stats.record_asr(t.asr_ms);
        if t.text.is_empty() {
            // 段落沒有文字(雜音):把可能殘留的預覽清掉
            let _ = events.send(PipelineEvent::Partial { text: String::new(), lang: Lang::Unknown, speaker: None });
            continue;
        }
        // 自動偵測信心低(Unknown)時沿用上一句語言。
        if t.lang == Lang::Unknown {
            if let Some(l) = last_lang {
                t.lang = l;
            }
        }
        if t.lang != Lang::Unknown {
            last_lang = Some(t.lang);
        }

        // 說話者
        let sp_t0 = Instant::now();
        let speaker = {
            let mut guard = speakers.lock().unwrap();
            // 只用語音本體:前段補白可能吃到上一位講者的尾音
            guard.as_mut().and_then(|s| s.identify(&pcm[seg.core_range()], seg.end_at))
        };
        if let Some(s) = speakers.lock().unwrap().as_mut() {
            stats.record_speaker(sp_t0.elapsed().as_millis() as u32);
            for (from, into) in s.take_merges() {
                let _ = events.send(PipelineEvent::SpeakerMerged { from, into });
            }
            let n = s.speakers().len();
            if n != known_speakers {
                known_speakers = n;
                let _ = events.send(PipelineEvent::Speakers { names: s.speakers() });
            }
        }

        // 斷句自適應:觀察值回饋給 vad thread
        let chars = if t.lang.is_cjk() { t.text.chars().count() } else { t.text.split_whitespace().count() };
        let incomplete = completeness(&t.text, t.lang) == Completeness::Incomplete;
        let _ = obs_tx.send(SegObservation {
            lang: t.lang,
            stat: SegStat { audio_ms: core_ms, chars, hit_max, incomplete },
        });

        let will_translate = t.lang != Lang::Zh
            && (cfg.allowed_langs.is_empty() || cfg.allowed_langs.contains(&t.lang));
        let _ = events.send(PipelineEvent::Transcript {
            id,
            text: t.text.clone(),
            lang: t.lang,
            asr_ms: t.asr_ms,
            will_translate,
            speaker: speaker.clone(),
            start_ms: samples_to_ms(start_sample),
            end_ms: samples_to_ms(end_sample),
        });
        // 先進回修帳本,再進 MT(之後的 Merged / Translated 才找得到這句)
        if let Some(tx) = &refine_tx {
            let _ = tx.send(RefineMsg::Sentence {
                id,
                start: start_sample,
                end: end_sample,
                text: t.text.clone(),
                lang: t.lang,
                speaker: speaker.clone(),
                will_translate,
            });
        }
        if will_translate {
            PipelineStats::add(&stats.mt_sent, 1);
            let _ = utt_tx.send(Utterance {
                id,
                text: t.text,
                lang: t.lang,
                speaker,
                start_sample,
                end_sample,
                timing,
                arrived: Instant::now(),
            });
        }
    }
    tracing::info!("asr thread 結束");
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

/// 這句的等待策略:None = 立刻翻;Some((最長等待, 靜音多久就翻))。
fn wait_policy(cfg: &PipelineConfig, u: &Utterance) -> Option<(Duration, Duration)> {
    if merge_limit_reached(cfg, u) {
        return None;
    }
    match completeness(&u.text, u.lang) {
        Completeness::Complete => None,
        Completeness::Incomplete => Some((cfg.incomplete_wait, cfg.incomplete_silence)),
        Completeness::Unknown => Some((cfg.unknown_wait, cfg.unknown_silence)),
    }
}

fn merge(
    cfg: &PipelineConfig,
    mut p: Utterance,
    u: Utterance,
    events: &mpsc::UnboundedSender<PipelineEvent>,
    refine_tx: &Option<crossbeam_channel::Sender<RefineMsg>>,
) -> Utterance {
    let sep = if p.lang.is_cjk() && u.lang.is_cjk() { "" } else { " " };
    p.text = format!("{}{}{}", p.text, sep, u.text);
    p.start_sample = p.start_sample.min(u.start_sample);
    p.end_sample = p.end_sample.max(u.end_sample);
    // 保留較早句的 timing 原點(延遲算最壞情況),音訊長度累加。
    p.timing.audio_ms += u.timing.audio_ms;
    if let (Some(a), Some(b)) = (p.timing.asr_done, u.timing.asr_done) {
        p.timing.asr_done = Some(a.max(b));
    }
    p.arrived = u.arrived;
    if p.lang == Lang::Unknown {
        p.lang = u.lang;
    }
    if p.speaker.is_none() {
        p.speaker = u.speaker; // 合併句沿用第一段的說話者,第一段沒有才用後段的
    }
    let _ = events.send(PipelineEvent::TranscriptMerged {
        into: p.id,
        from: u.id,
        text: p.text.clone(),
        start_ms: samples_to_ms(p.start_sample),
        end_ms: samples_to_ms(p.end_sample),
    });
    if let Some(tx) = refine_tx {
        let _ = tx.send(RefineMsg::Merged {
            into: p.id,
            from: u.id,
            text: p.text.clone(),
            start: p.start_sample,
            end: p.end_sample,
        });
    }
    let _ = cfg;
    p
}

async fn mt_loop(
    cfg: PipelineConfig,
    translator: Option<Arc<dyn Translator>>,
    mut rx: mpsc::UnboundedReceiver<Utterance>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    stats: Arc<PipelineStats>,
    refine_tx: Option<crossbeam_channel::Sender<RefineMsg>>,
) {
    let Some(translator) = translator else {
        // 無翻譯器:把 utterance 吃掉即可。
        while rx.recv().await.is_some() {
            PipelineStats::add(&stats.mt_done, 1);
        }
        return;
    };
    let mut context: VecDeque<(String, String)> = VecDeque::new();
    let mut pending: Option<Utterance> = None;

    loop {
        let next = if let Some(p) = &pending {
            // 暫存句等待中:最長等 budget;但講者已靜音 ≥ silence 就不等了(對方大概說完了)。
            let (budget, silence) = wait_policy(&cfg, p).unwrap_or((Duration::ZERO, Duration::ZERO));
            let waited = p.arrived.elapsed();
            let quiet = Duration::from_millis(stats.silence_ms());
            if waited >= budget || quiet >= silence {
                let u = pending.take().unwrap();
                translate_one(&cfg, &translator, &events, &mut context, u, &stats, &refine_tx).await;
                continue;
            }
            // 每 100 ms 看一次靜音狀態
            let slice = (budget - waited).min(Duration::from_millis(100));
            match tokio::time::timeout(slice, rx.recv()).await {
                Ok(Some(u)) => Some(u),
                Ok(None) => None, // channel 關閉
                Err(_) => continue,
            }
        } else {
            rx.recv().await
        };

        let Some(u) = next else {
            if let Some(p) = pending.take() {
                translate_one(&cfg, &translator, &events, &mut context, p, &stats, &refine_tx).await;
            }
            break;
        };

        let u = match pending.take() {
            Some(p) => {
                PipelineStats::add(&stats.merges, 1);
                PipelineStats::add(&stats.mt_done, 1); // 被併入的那句不會單獨翻
                merge(&cfg, p, u, &events, &refine_tx)
            }
            None => u,
        };

        // 合併後重新判斷:還沒說完就繼續等,說完或達上限就翻。
        if wait_policy(&cfg, &u).is_some() {
            pending = Some(u);
        } else {
            translate_one(&cfg, &translator, &events, &mut context, u, &stats, &refine_tx).await;
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
    stats: &PipelineStats,
    refine_tx: &Option<crossbeam_channel::Sender<RefineMsg>>,
) {
    let hits: Vec<_> = cfg.glossary.hits(&u.text).into_iter().cloned().collect();
    let req = TranslateRequest {
        text: u.text.clone(),
        src_lang: u.lang,
        context: context.iter().cloned().collect(),
        glossary: hits.clone(),
        lookahead: Vec::new(),
        // 有回修時固定用 slot 0,回修用 slot 1,前綴快取互不干擾
        slot: if refine_tx.is_some() { Some(0) } else { None },
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
            let snap = u.timing.snapshot();
            stats.record_mt(res.first_token_ms.unwrap_or(res.total_ms), res.total_ms, snap.e2e_first_ms);
            context.push_back((u.text.clone(), text.clone()));
            // 累加式前文:平時只追加(提示前綴不變 → 快取命中);超過額度才一次裁到最近幾句。
            let total: usize = context.iter().map(|(s, t)| s.chars().count() + t.chars().count()).sum();
            if total > cfg.context_max_chars {
                while context.len() > cfg.context_sentences {
                    context.pop_front();
                }
            }
            if let Some(tx) = refine_tx {
                let _ = tx.send(RefineMsg::Translated { id, text: text.clone() });
            }
            let _ = events.send(PipelineEvent::TranslationDone {
                id,
                source: u.text,
                text,
                timing: snap,
                speaker: u.speaker,
            });
        }
        Ok(Err(e)) => {
            PipelineStats::add(&stats.mt_done, 1);
            if let Some(tx) = refine_tx {
                let _ = tx.send(RefineMsg::Failed { id });
            }
            let _ = events.send(PipelineEvent::TranslationFailed {
                id,
                error: format!("{e:#}"),
            });
        }
        Err(_) => {
            PipelineStats::add(&stats.mt_done, 1);
            if let Some(tx) = refine_tx {
                let _ = tx.send(RefineMsg::Failed { id });
            }
            let _ = events.send(PipelineEvent::TranslationFailed {
                id,
                error: format!("翻譯逾時(> {:?})", cfg.mt_timeout),
            });
        }
    }
}
