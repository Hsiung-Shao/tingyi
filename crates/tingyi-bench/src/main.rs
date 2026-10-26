//! tingyi-bench:技術驗證週的量測 CLI。
//!
//! 子命令:
//! - `devices`           列出預設輸出裝置與有音訊 session 的程式
//! - `capture-probe`     擷取探測:每秒封包數、靜音封包、不連續次數、樣本數/實際時間比
//! - `asr-eval`          CER/WER 評測(scripts/fetch-eval.py 下載的評測集):逐句或串流模式
//! - `download-model`    下載專用模型(與 app 內下載同一套:固定 commit、續傳、SHA256 校驗)
//! - `vad`               播影片,印出 VAD 切段
//! - `asr`               播影片,印出辨識結果與延遲
//! - `mt`                固定句子集跑翻譯,輸出對照表(可指定多個模型)
//! - `live`              完整管線即時量測,輸出 JSONL 與統計
//! - `serve`             只啟動 llama-server(除錯用)

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tingyi_core::asr::Lang;
use tingyi_core::audio::capture::{default_render_device_name, start_capture, start_loopback, CaptureConfig, CaptureStats};
use tingyi_core::audio::source::{self, AudioSource, SourceControl};
use tingyi_core::llama::{LlamaServerProcess, LlamaServerSpec};
use tingyi_core::metrics::{Summary, TimingSnapshot};
use tingyi_core::mt::{LlamaServerConfig, LlamaServerTranslator, TranslateRequest, Translator};
use tingyi_core::pipeline::{self, PipelineConfig, PipelineEvent};
use tingyi_core::vad::{Segmenter, VadConfig};
use tingyi_core::{gpu, paths};

#[derive(Parser)]
#[command(name = "tingyi-bench", about = "聽譯量測工具")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Devices,
    /// 擷取探測:每秒印封包、frame、靜音封包、不連續次數與樣本數/實際時間比
    CaptureProbe {
        /// `system` 或 `app:<pid>`
        #[arg(long, default_value = "system")]
        source: String,
        #[arg(long, default_value_t = 60)]
        seconds: u64,
    },
    Vad {
        #[arg(long, default_value_t = 60)]
        seconds: u64,
    },
    Asr {
        #[arg(long, default_value_t = 120)]
        seconds: u64,
        /// 鎖定語言 en/ja/ko
        #[arg(long)]
        lang: Option<String>,
        #[arg(long, default_value_t = 4)]
        threads: i32,
        /// 輸出 JSONL
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Mt {
        /// 每行一句,格式 `lang<TAB>text`(lang = en/ja/ko)
        #[arg(long)]
        sentences: PathBuf,
        /// 模型名稱(可多個),例如 Hy-MT2-1.8B-Q8_0
        #[arg(long, num_args = 1..)]
        models: Vec<String>,
        #[arg(long, default_value_t = 8178)]
        port: u16,
        /// 帶前文(前 N 句)
        #[arg(long, default_value_t = 0)]
        context: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Live {
        #[arg(long, default_value_t = 180)]
        seconds: u64,
        #[arg(long, default_value = "Hy-MT2-1.8B-Q8_0")]
        model: String,
        #[arg(long, default_value_t = 8178)]
        port: u16,
        /// 不自己啟動 llama-server,直接連現有的
        #[arg(long)]
        attach: bool,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
        /// 把每段原始 16 kHz 音訊存成 wav 到此目錄(離線 A/B 用)
        #[arg(long)]
        record: Option<PathBuf>,
        /// 關閉送辨識前的音量正規化
        #[arg(long)]
        no_normalize: bool,
        /// 正式辨識引擎:sensevoice(預設)或 whisper(自動啟動 whisper-server 於 port+1)
        #[arg(long, default_value = "sensevoice")]
        asr: String,
        /// 把整條連續 16 kHz 音訊寫成一個 wav(回修對照用)
        #[arg(long)]
        record_all: Option<PathBuf>,
        /// 關閉回修(慢路徑)
        #[arg(long)]
        no_refine: bool,
        /// 回修用獨立 whisper-server 的網址(預設與快路徑共用)
        #[arg(long)]
        refine_server: Option<String>,
    },
    Serve {
        #[arg(long, default_value = "Hy-MT2-1.8B-Q8_0")]
        model: String,
        #[arg(long, default_value_t = 8178)]
        port: u16,
    },
    /// 對 wav 檔做辨識(離線 smoke test;自動重取樣到 16 kHz)
    AsrFile {
        files: Vec<PathBuf>,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long, default_value_t = 4)]
        threads: i32,
        /// 送辨識前做音量正規化(與不帶旗標比較,做 A/B)
        #[arg(long)]
        normalize: bool,
        /// 辨識引擎:sensevoice(預設)或 whisper(自動啟動 whisper-server)
        #[arg(long, default_value = "sensevoice")]
        engine: String,
    },
    /// 說話者 embedding 驗證:印相似度矩陣與分群結果(不同檔應為不同人)
    Speakers { files: Vec<PathBuf> },
    /// 下載專用模型(與 app 內下載同一套:固定 commit、續傳、SHA256 校驗)
    DownloadModel {
        /// reazonspeech 或 parakeet
        id: String,
        /// 模型目錄(預設為資料目錄的 models)
        #[arg(long)]
        models_dir: Option<PathBuf>,
    },
    /// CER/WER 評測:每集 micro 錯誤率、語言判斷正確率、延遲、RTF;逐句結果寫 JSONL
    AsrEval {
        /// 評測集(資料目錄 eval/<名>/manifest.tsv)
        #[arg(long, num_args = 1.., default_values_t = ["ja-rs".to_string(), "ja-jsut".into(), "ja-fleurs".into(), "en-fleurs".into(), "ko-fleurs".into()])]
        sets: Vec<String>,
        /// 引擎(可多個):sensevoice、whisper、reazonspeech(ja-en)、reazonspeech-ja、reazonspeech-pad(段首尾墊 0.35 s)、parakeet、routed
        #[arg(long, num_args = 1.., default_values_t = ["sensevoice".to_string()])]
        engines: Vec<String>,
        /// 每集最多幾句(0 = 全部)
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long, default_value_t = 4)]
        threads: i32,
        /// 不鎖定語言,改自動偵測(並統計語言判斷正確率)
        #[arg(long)]
        auto: bool,
        /// 關閉送辨識前的音量正規化(預設與管線相同,開啟)
        #[arg(long)]
        no_normalize: bool,
        #[arg(long, default_value = "bench-out/eval")]
        out: PathBuf,
        /// 串流模式:每 `session-size` 句接成一條長音訊(固定種子間隔 0.3–1.5 s、底噪 -60 dBFS),
        /// 走完整管線(VAD、正式辨識;不預覽、不回修、不翻譯),量串接後的錯誤率與切段診斷
        #[arg(long)]
        stream: bool,
        #[arg(long, default_value_t = 20)]
        session_size: usize,
        /// 串流模式:段首往前補的緩衝(毫秒,預設依引擎:SenseVoice/專用模型 800、Whisper 200)
        #[arg(long)]
        pre_pad_ms: Option<u32>,
        /// 串流模式:關閉幻聽重複壓縮
        #[arg(long)]
        no_cleanup: bool,
        /// 串流模式:單段上限秒數(預設沿用 VadConfig;搭配 --no-tuning 才不會被調整器覆寫)
        #[arg(long)]
        max_speech_sec: Option<f32>,
        /// 串流模式:關閉斷句參數調整(全程固定 VAD 參數)
        #[arg(long)]
        no_tuning: bool,
        /// 串流模式:每句先調到這個響度(RMS dBFS)再接成長音訊;評測集裡有低到 -66 dBFS 的錄音,
        /// 不調的話會和底噪一樣小聲,不像真實播放
        #[arg(long, default_value_t = -26.0, allow_hyphen_values = true)]
        level_dbfs: f32,
        /// 串流模式:專用模型段首段尾墊的合成靜音毫秒(routed 引擎)
        #[arg(long)]
        transducer_pad_ms: Option<u32>,
    },
}

/// 建立正式辨識引擎;whisper 時順便啟動 whisper-server(回傳的 process 要活到辨識結束)。
fn make_engine(
    engine: &str,
    threads: i32,
    lock: Option<Lang>,
    port: u16,
) -> Result<(Box<dyn tingyi_core::asr::Transcriber>, Option<tingyi_core::llama::WhisperServerProcess>)> {
    use tingyi_core::asr::{SenseVoice, SenseVoiceConfig, SherpaTransducer, TransducerConfig, WhisperServer, WhisperServerConfig};
    use tingyi_core::llama::{WhisperServerProcess, WhisperServerSpec};
    let transducer = |cfg: TransducerConfig| -> Result<Box<dyn tingyi_core::asr::Transcriber>> {
        let t0 = Instant::now();
        let t = SherpaTransducer::new(&cfg)?;
        println!("{} 載入 {} ms", cfg.name, t0.elapsed().as_millis());
        Ok(Box::new(t))
    };
    match engine {
        "reazonspeech" => Ok((transducer(TransducerConfig::reazonspeech(paths::REAZONSPEECH_JA_EN_DIR_NAME, threads))?, None)),
        "reazonspeech-ja" => Ok((transducer(TransducerConfig::reazonspeech(paths::REAZONSPEECH_JA_DIR_NAME, threads))?, None)),
        "reazonspeech-pad" => {
            let mut cfg = TransducerConfig::reazonspeech(paths::REAZONSPEECH_JA_EN_DIR_NAME, threads);
            cfg.pad_ms = 350;
            Ok((transducer(cfg)?, None))
        }
        "parakeet" => Ok((transducer(TransducerConfig::parakeet(threads))?, None)),
        "routed" => {
            let t0 = Instant::now();
            let load = [Lang::En, Lang::Ja, Lang::Ko].into_iter().collect();
            let sv = SenseVoiceConfig { num_threads: threads, language: lock, ..Default::default() };
            let r = tingyi_core::route::RoutedTranscriber::new(&sv, &load, paths::REAZONSPEECH_JA_DIR_NAME, 0)?;
            println!("routed 載入 {} ms({:?})", t0.elapsed().as_millis(), r.available());
            Ok((Box::new(r), None))
        }
        "whisper" => {
            let before = gpu::nvidia_used_mib();
            let mut spec = WhisperServerSpec::new(paths::whisper_model(), port);
            spec.log_file = Some(paths::data_dir().join("whisper-server.log"));
            let mut p = WhisperServerProcess::spawn(spec)?;
            let t = p.wait_ready_blocking(Duration::from_secs(120))?;
            let after = gpu::nvidia_used_mib();
            println!(
                "whisper-server 就緒 {:.1}s,顯示記憶體增量 {:?} MiB",
                t.as_secs_f32(),
                after.zip(before).map(|(a, b)| a.saturating_sub(b))
            );
            let w = WhisperServer::new(WhisperServerConfig {
                base_url: p.base_url(),
                language: lock,
                ..Default::default()
            });
            Ok((Box::new(w), Some(p)))
        }
        _ => {
            let t0 = Instant::now();
            let s = SenseVoice::new(&SenseVoiceConfig {
                num_threads: threads,
                language: lock,
                ..Default::default()
            })?;
            println!("SenseVoice 載入 {} ms", t0.elapsed().as_millis());
            Ok((Box::new(s), None))
        }
    }
}

fn run_download_model(id: &str, models_dir: Option<PathBuf>) -> Result<()> {
    use tingyi_core::model_download::{self as md, DownloadCtl, DownloadOptions};
    let spec = md::spec(id).ok_or_else(|| anyhow!("未知的模型 {id}(可用:reazonspeech、parakeet)"))?;
    let dir = models_dir.unwrap_or_else(paths::models_dir);
    std::fs::create_dir_all(&dir)?;
    println!("下載 {}({} MB)到 {}", spec.label, spec.total_size() / 1_000_000, dir.display());
    md::check_disk_space(spec, &dir).map_err(|e| anyhow!("{e}"))?;
    let t0 = Instant::now();
    let ctl = DownloadCtl::default();
    md::download_model(spec, &dir, &DownloadOptions::default(), &ctl, &mut |p| {
        println!("  {:?} {:>5.1}% {:>7.1} MB/s{}", p.phase, p.done as f64 * 100.0 / p.total.max(1) as f64, p.bps as f64 / 1e6,
            p.message.map(|m| format!("  {m}")).unwrap_or_default());
    })
    .map_err(|e| anyhow!("{e}"))?;
    println!("完成,{:.1} s;狀態:{:?}", t0.elapsed().as_secs_f32(), md::model_status(spec, &dir));
    Ok(())
}

/// 評測集的一句。
struct EvalUtt {
    wav: PathBuf,
    lang: Lang,
    reference: String,
}

fn load_eval_set(name: &str, limit: usize) -> Result<Vec<EvalUtt>> {
    let dir = paths::data_dir().join("eval").join(name);
    let manifest = dir.join("manifest.tsv");
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("讀不到 {}(先跑 scripts/fetch-eval.py)", manifest.display()))?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            return Err(anyhow!("{} 格式錯誤:{line}", manifest.display()));
        }
        out.push(EvalUtt { wav: dir.join(cols[0]), lang: parse_lang(cols[1])?, reference: cols[2].to_string() });
        if limit > 0 && out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn read_wav_16k(path: &std::path::Path) -> Result<Vec<f32>> {
    let w = sherpa_onnx::Wave::read(&path.to_string_lossy()).ok_or_else(|| anyhow!("讀不到 wav:{}", path.display()))?;
    let samples = w.samples().to_vec();
    if w.sample_rate() == 16000 {
        return Ok(samples);
    }
    let r = sherpa_onnx::LinearResampler::create(w.sample_rate(), 16000).ok_or_else(|| anyhow!("resampler"))?;
    Ok(r.resample(&samples, true))
}

#[derive(serde::Serialize)]
struct EvalSummary {
    engine: String,
    set: String,
    n: usize,
    /// 英文為 WER,其餘為 CER(micro 平均)。
    error_rate: f64,
    ops: tingyi_core::eval::EditOps,
    /// 自動模式才有:判斷出的語言與標註相同的比例。
    lid_accuracy: Option<f64>,
    asr_ms: tingyi_core::metrics::Stats,
    rtf: f64,
}

fn run_asr_eval(sets: &[String], engines: &[String], limit: usize, threads: i32, auto: bool, normalize: bool, out: &std::path::Path) -> Result<()> {
    use tingyi_core::eval::{score, EditOps};
    std::fs::create_dir_all(out)?;
    let data: Vec<(String, Vec<EvalUtt>)> = sets.iter().map(|s| Ok((s.clone(), load_eval_set(s, limit)?))).collect::<Result<_>>()?;
    let mut summaries = Vec::new();
    for engine in engines {
        // 鎖定模式每集各建一個鎖定該語言的引擎(SenseVoice 的語言只能在建立時設定)
        let mut shared = if auto { Some(make_engine(engine, threads, None, 8179)?) } else { None };
        for (set, utts) in &data {
            let lock = utts.first().map(|u| u.lang);
            let per_set;
            let (asr, _server) = match shared.as_mut() {
                Some(e) => (&e.0, &e.1),
                None => {
                    per_set = make_engine(engine, threads, lock, 8179)?;
                    (&per_set.0, &per_set.1)
                }
            };
            let mut f = std::fs::File::create(out.join(format!("{engine}-{set}{}.jsonl", if auto { "-auto" } else { "" })))?;
            let mut corpus = EditOps::default();
            let (mut lid_ok, mut ms, mut audio_ms_total, mut asr_ms_total) = (0usize, Vec::new(), 0u64, 0u64);
            for (i, u) in utts.iter().enumerate() {
                let mut pcm = read_wav_16k(&u.wav)?;
                let audio_ms = (pcm.len() * 1000 / 16000) as u64;
                if normalize {
                    pcm = tingyi_core::audio::normalize_rms(&pcm, -20.0, 8.0);
                }
                let t = asr.transcribe(&pcm, if auto { None } else { Some(u.lang) })?;
                let ops = score(&u.reference, &t.text, u.lang);
                corpus.add(&ops);
                lid_ok += usize::from(t.lang == u.lang);
                ms.push(t.asr_ms);
                audio_ms_total += audio_ms;
                asr_ms_total += t.asr_ms as u64;
                serde_json::to_writer(&mut f, &serde_json::json!({
                    "i": i, "wav": u.wav.file_name().map(|x| x.to_string_lossy().to_string()),
                    "ref": u.reference, "hyp": t.text, "lang_ref": u.lang, "lang_hyp": t.lang,
                    "ops": ops, "rate": ops.rate(), "asr_ms": t.asr_ms, "audio_ms": audio_ms,
                }))?;
                writeln!(f)?;
            }
            let s = EvalSummary {
                engine: engine.clone(),
                set: set.clone(),
                n: utts.len(),
                error_rate: corpus.rate(),
                ops: corpus,
                lid_accuracy: auto.then(|| lid_ok as f64 / utts.len().max(1) as f64),
                asr_ms: tingyi_core::metrics::Stats::from_samples(ms),
                rtf: asr_ms_total as f64 / audio_ms_total.max(1) as f64,
            };
            println!(
                "{engine:<12} {set:<10} n={:<4} {}={:>6.2}%  (替換 {} 刪除 {} 插入 {} / {})  {}辨識 p50 {} / p95 {} ms  RTF {:.3}",
                s.n,
                if utts.first().is_some_and(|u| u.lang == Lang::En) { "WER" } else { "CER" },
                s.error_rate * 100.0,
                s.ops.sub,
                s.ops.del,
                s.ops.ins,
                s.ops.ref_len,
                s.lid_accuracy.map(|a| format!("語言判斷 {:.1}%  ", a * 100.0)).unwrap_or_default(),
                s.asr_ms.p50,
                s.asr_ms.p95,
                s.rtf
            );
            summaries.push(s);
        }
    }
    let path = out.join(format!("summary-{}.json", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()));
    std::fs::write(&path, serde_json::to_string_pretty(&summaries)?)?;
    println!("彙總:{}", path.display());
    Ok(())
}

/// 串流評測的 A/B 開關。
struct StreamOpts {
    session_size: usize,
    pre_pad_ms: Option<u32>,
    cleanup: bool,
    max_speech_sec: Option<f32>,
    tuning: bool,
    level_dbfs: f32,
    transducer_pad_ms: Option<u32>,
}

/// 把整段縮放到指定 RMS(dBFS),clip 在 ±1。
fn level_to(pcm: &[f32], dbfs: f32) -> Vec<f32> {
    let rms = (pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
    if rms < 1e-7 {
        return pcm.to_vec();
    }
    let gain = 10f32.powf(dbfs / 20.0) / rms;
    pcm.iter().map(|x| (x * gain).clamp(-1.0, 1.0)).collect()
}

#[derive(serde::Serialize, Default)]
struct StreamSummary {
    engine: String,
    set: String,
    sessions: usize,
    utts: usize,
    segments: usize,
    error_rate: f64,
    ops: tingyi_core::eval::EditOps,
    /// 依時間重疊分組:一對一、切開、合併、漏掉、多出
    matched: usize,
    split: usize,
    merged: usize,
    missed: usize,
    inserted: usize,
    asr_ms: tingyi_core::metrics::Stats,
    /// VAD 切出的段落總長 ÷ 參考句總長(含補白,> 1 代表多切了靜音;明顯 < 1 代表語音沒進辨識)
    vad_coverage: f64,
    /// 辨識段時間戳比前一段早(VAD 重建原點錯誤的回歸檢查,應為 0)
    ts_backwards: usize,
}

/// 串流評測:句子接成長音訊走完整管線,主指標是整條 session 串接後的錯誤率(不受切段影響)。
async fn run_stream_eval(
    sets: &[String],
    engines: &[String],
    limit: usize,
    threads: i32,
    auto: bool,
    opts: &StreamOpts,
    out: &std::path::Path,
) -> Result<()> {
    use tingyi_core::audio::capture::AudioChunk;
    use tingyi_core::eval::{build_stream, group_alignment, score, EditOps, GroupKind};
    use tingyi_core::pipeline::AsrEngine;
    std::fs::create_dir_all(out)?;
    let mut summaries = Vec::new();
    for engine in engines {
        let (asr, server) = make_engine(engine, threads, None, 8179)?;
        drop(asr); // 管線自己建引擎;這裡只為了讓 whisper-server 活著
        let asr_engine = match (engine.as_str(), &server) {
            (_, Some(p)) => AsrEngine::Whisper { base_url: p.base_url() },
            ("sensevoice", None) => AsrEngine::SenseVoice,
            ("routed", None) => AsrEngine::Routed,
            (other, None) => return Err(anyhow!("串流模式不支援引擎 {other}(可用 sensevoice、whisper、routed)")),
        };
        let tag = format!(
            "{engine}{}{}{}{}{}{}",
            if opts.level_dbfs == -26.0 { String::new() } else { format!("-lv{}", opts.level_dbfs) },
            opts.transducer_pad_ms.map(|ms| format!("-tpad{ms}")).unwrap_or_default(),
            opts.pre_pad_ms.map(|ms| format!("-pre{ms}")).unwrap_or_default(),
            if opts.cleanup { "" } else { "-nocleanup" },
            opts.max_speech_sec.map(|s| format!("-max{s}")).unwrap_or_default(),
            if opts.tuning { "" } else { "-notune" }
        );
        for set in sets {
            let utts = load_eval_set(set, limit)?;
            let lang = utts.first().map(|u| u.lang).unwrap_or(Lang::Unknown);
            let sep = if lang == Lang::En { " " } else { "" };
            let mut f = std::fs::File::create(out.join(format!("stream-{tag}-{set}{}.jsonl", if auto { "-auto" } else { "" })))?;
            let mut s = StreamSummary { engine: tag.clone(), set: set.clone(), ..Default::default() };
            let mut corpus = EditOps::default();
            let mut asr_ms = Vec::new();
            let (mut seg_audio_ms, mut ref_audio_ms) = (0u64, 0u64);
            for (k, group) in utts.chunks(opts.session_size.max(1)).enumerate() {
                let pcms: Vec<Vec<f32>> = group.iter().map(|u| Ok(level_to(&read_wav_16k(&u.wav)?, opts.level_dbfs))).collect::<Result<_>>()?;
                let (audio, spans) = build_stream(&pcms, k as u64, (300, 1500), -60.0, 16_000);

                let mut cfg = PipelineConfig::default();
                cfg.asr_engine = asr_engine.clone();
                cfg.asr.num_threads = threads;
                cfg.preview = false;
                cfg.speaker_id = false;
                cfg.refine.enabled = false;
                cfg.cleanup = opts.cleanup;
                if let Some(ms) = opts.pre_pad_ms {
                    cfg.vad.pre_pad_ms = Some(ms);
                }
                if let Some(s) = opts.max_speech_sec {
                    cfg.vad.max_speech_sec = s;
                }
                cfg.seg_tuning = opts.tuning;
                if let Some(ms) = opts.transducer_pad_ms {
                    cfg.transducer_pad_ms = ms;
                }
                cfg.lock_lang = (!auto).then_some(lang);
                if cfg.lock_lang.is_some() {
                    cfg.allowed_langs = HashSet::new();
                }
                let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
                let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioChunk>(256);
                let handle = pipeline::start_with_audio(cfg, None, ev_tx, audio_rx)?;
                let collector = tokio::spawn(async move {
                    let mut v = Vec::new();
                    while let Some(e) = ev_rx.recv().await {
                        v.push(e);
                    }
                    v
                });
                tokio::task::spawn_blocking(move || {
                    for c in audio.chunks(480) {
                        if audio_tx.send(AudioChunk { samples: c.to_vec(), gen: 0 }).is_err() {
                            break;
                        }
                    }
                })
                .await?;
                handle.finish().await;
                let events = collector.await?;

                let mut hyps: Vec<(String, (usize, usize))> = Vec::new();
                for e in &events {
                    match e {
                        PipelineEvent::Transcript { text, start_ms, end_ms, asr_ms: ms, .. } => {
                            hyps.push((text.clone(), (*start_ms as usize * 16, *end_ms as usize * 16)));
                            asr_ms.push(*ms);
                        }
                        PipelineEvent::Segment { audio_ms, .. } => seg_audio_ms += *audio_ms as u64,
                        _ => {}
                    }
                }
                ref_audio_ms += spans.iter().map(|(a, b)| ((b - a) / 16) as u64).sum::<u64>();
                let reference = group.iter().map(|u| u.reference.as_str()).collect::<Vec<_>>().join(sep);
                let hyp = hyps.iter().map(|h| h.0.as_str()).collect::<Vec<_>>().join(sep);
                let ops = score(&reference, &hyp, lang);
                corpus.add(&ops);
                let hyp_spans: Vec<(usize, usize)> = hyps.iter().map(|h| h.1).collect();
                s.ts_backwards += hyp_spans.windows(2).filter(|w| w[1].0 < w[0].0).count();
                let groups = group_alignment(&spans, &hyp_spans, 3_200);
                for g in &groups {
                    match g.kind {
                        GroupKind::Match => s.matched += 1,
                        GroupKind::Split => s.split += 1,
                        GroupKind::Merged => s.merged += 1,
                        GroupKind::Missed => s.missed += 1,
                        GroupKind::Inserted => s.inserted += 1,
                    }
                }
                s.sessions += 1;
                s.utts += group.len();
                s.segments += hyps.len();
                serde_json::to_writer(&mut f, &serde_json::json!({
                    "session": k, "ref": reference, "hyp": hyp, "ops": ops, "rate": ops.rate(),
                    "segments": hyp_spans, "ref_spans": spans, "groups": groups,
                }))?;
                writeln!(f)?;
            }
            s.error_rate = corpus.rate();
            s.ops = corpus;
            s.asr_ms = tingyi_core::metrics::Stats::from_samples(asr_ms);
            s.vad_coverage = seg_audio_ms as f64 / ref_audio_ms.max(1) as f64;
            println!(
                "[串流] {tag:<16} {set:<10} {} 條/{} 句 → {} 段(覆蓋 {:.0}%、時間倒退 {})  {}={:>6.2}%(替換 {} 刪除 {} 插入 {} / {})  一對一 {} 切開 {} 合併 {} 漏掉 {} 多出 {}  辨識 p50 {} ms",
                s.sessions,
                s.utts,
                s.segments,
                s.vad_coverage * 100.0,
                s.ts_backwards,
                if lang == Lang::En { "WER" } else { "CER" },
                s.error_rate * 100.0,
                s.ops.sub,
                s.ops.del,
                s.ops.ins,
                s.ops.ref_len,
                s.matched,
                s.split,
                s.merged,
                s.missed,
                s.inserted,
                s.asr_ms.p50
            );
            summaries.push(s);
        }
    }
    let path = out.join(format!("stream-summary-{}.json", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()));
    std::fs::write(&path, serde_json::to_string_pretty(&summaries)?)?;
    println!("彙總:{}", path.display());
    Ok(())
}

fn parse_lang(s: &str) -> Result<Lang> {
    Ok(match s {
        "en" => Lang::En,
        "ja" => Lang::Ja,
        "ko" => Lang::Ko,
        "zh" => Lang::Zh,
        _ => return Err(anyhow!("不支援的語言:{s}")),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    println!("資料目錄:{}", paths::data_dir().display());
    match cli.cmd {
        Cmd::Devices => {
            println!("預設輸出裝置:{}", default_render_device_name()?);
            for e in source::list_audio_sources()? {
                let group = match e.group {
                    source::SourceGroup::Playing => "播放中",
                    source::SourceGroup::Windowed => "有視窗",
                    source::SourceGroup::Background => "背景",
                };
                let label = match &e.kind {
                    source::SourceKind::Window { title, .. } => format!("[{}]: {title}", e.exe),
                    source::SourceKind::Background { name } => format!("{name}({})", e.exe),
                };
                println!("  [{group}] pid {:>6}  {label}", e.pid);
            }
            if let Some((n, t)) = gpu::nvidia_info() {
                println!("GPU:{n} {t} MiB,目前已用 {:?} MiB", gpu::nvidia_used_mib());
            }
        }
        Cmd::CaptureProbe { source, seconds } => run_capture_probe(&source, seconds)?,
        Cmd::Vad { seconds } => run_vad(seconds)?,
        Cmd::Asr {
            seconds,
            lang,
            threads,
            out,
        } => {
            let lock = lang.as_deref().map(parse_lang).transpose()?;
            run_pipeline(seconds, lock, threads, None, out, None, true, tingyi_core::pipeline::AsrEngine::SenseVoice, None, tingyi_core::refine::RefineConfig { enabled: false, ..Default::default() }).await?;
        }
        Cmd::Mt {
            sentences,
            models,
            port,
            context,
            out,
        } => run_mt(sentences, models, port, context, out).await?,
        Cmd::Live {
            seconds,
            model,
            port,
            attach,
            lang,
            out,
            record,
            no_normalize,
            asr,
            record_all,
            no_refine,
            refine_server,
        } => {
            let lock = lang.as_deref().map(parse_lang).transpose()?;
            let refine_on = !no_refine && asr == "whisper";
            // whisper 引擎:先啟動 whisper-server(活到量測結束)
            let mut whisper_proc = None;
            let mut asr_engine = tingyi_core::pipeline::AsrEngine::SenseVoice;
            if asr == "whisper" {
                let (_, p) = make_engine("whisper", 4, lock, port + 1)?;
                let p = p.expect("whisper process");
                asr_engine = tingyi_core::pipeline::AsrEngine::Whisper { base_url: p.base_url() };
                whisper_proc = Some(p);
            }
            let before = gpu::nvidia_used_mib();
            let mut server = None;
            if !attach {
                let mut spec = LlamaServerSpec::new(paths::gguf_model(&model), port);
                if refine_on {
                    spec = spec.with_refine();
                }
                spec.log_file = Some(paths::data_dir().join("llama-server.log"));
                let mut p = LlamaServerProcess::spawn(spec)?;
                let t = p.wait_ready_blocking(Duration::from_secs(120))?;
                println!("llama-server 就緒,載入 {:.1} 秒", t.as_secs_f32());
                server = Some(p);
            }
            let after = gpu::nvidia_used_mib();
            if let (Some(a), Some(b)) = (before, after) {
                println!("llama-server 顯示記憶體增量:{} MiB(載入後總用量 {} MiB)", b.saturating_sub(a), b);
            }
            let translator: Arc<dyn Translator> = Arc::new(LlamaServerTranslator::new(
                LlamaServerConfig::for_model(format!("http://127.0.0.1:{port}"), &model),
            ));
            // 熱身一句,讓固定前綴進快取。
            let mut noop = |_: &str| {};
            let _ = translator
                .translate(
                    TranslateRequest {
                        text: "Hello.".into(),
                        src_lang: Lang::En,
                        slot: if refine_on { Some(0) } else { None },
                        ..Default::default()
                    },
                    &mut noop,
                )
                .await;
            let refine = tingyi_core::refine::RefineConfig { enabled: refine_on, whisper_url: refine_server, ..Default::default() };
            run_pipeline(seconds, lock, 4, Some(translator), out, record, !no_normalize, asr_engine, record_all, refine).await?;
            if let Some(mut s) = server {
                println!("關閉 llama-server:{}", if s.shutdown() { "已結束" } else { "未確認" });
            }
            if let Some(mut w) = whisper_proc {
                println!("關閉 whisper-server:{}", if w.shutdown() { "已結束" } else { "未確認" });
            }
        }
        Cmd::AsrFile { files, lang, threads, normalize, engine } => {
            let lock = lang.as_deref().map(parse_lang).transpose()?;
            let (asr, _server) = make_engine(&engine, threads, lock, 8179)?;
            for f in files {
                let w = sherpa_onnx::Wave::read(&f.to_string_lossy())
                    .ok_or_else(|| anyhow!("讀不到 wav:{}", f.display()))?;
                let mut samples = w.samples().to_vec();
                if w.sample_rate() != 16000 {
                    let r = sherpa_onnx::LinearResampler::create(w.sample_rate(), 16000)
                        .ok_or_else(|| anyhow!("resampler"))?;
                    samples = r.resample(&samples, true);
                }
                let audio_ms = samples.len() * 1000 / 16000;
                let level = tingyi_core::audio::rms_dbfs(&samples);
                if normalize {
                    samples = tingyi_core::audio::normalize_rms(&samples, -20.0, 8.0);
                }
                print!("[{level:6.1} dBFS{}] ", if normalize { " →正規化" } else { "" });
                let t = asr.transcribe(&samples, lock)?;
                println!(
                    "{}: 音訊 {audio_ms} ms,辨識 {} ms(RTF {:.3})[{}] {}\n   raw: {}",
                    f.file_name().unwrap_or_default().to_string_lossy(),
                    t.asr_ms,
                    t.asr_ms as f32 / audio_ms as f32,
                    t.lang.code(),
                    t.text,
                    t.raw
                );
            }
        }
        Cmd::AsrEval {
            sets, engines, limit, threads, auto, no_normalize, out, stream, session_size, pre_pad_ms, no_cleanup, max_speech_sec, no_tuning, level_dbfs, transducer_pad_ms,
        } => {
            if stream {
                let opts = StreamOpts {
                    session_size, pre_pad_ms, cleanup: !no_cleanup, max_speech_sec, tuning: !no_tuning, level_dbfs, transducer_pad_ms,
                };
                run_stream_eval(&sets, &engines, limit, threads, auto, &opts, &out).await?
            } else {
                run_asr_eval(&sets, &engines, limit, threads, auto, !no_normalize, &out)?
            }
        }
        Cmd::DownloadModel { id, models_dir } => run_download_model(&id, models_dir)?,
        Cmd::Speakers { files } => {
            use tingyi_core::speaker::{SpeakerConfig, SpeakerIdentifier};
            let t0 = Instant::now();
            let mut id = SpeakerIdentifier::new(SpeakerConfig::default())?;
            println!("說話者模型載入 {} ms", t0.elapsed().as_millis());
            let mut embs: Vec<(String, Vec<f32>)> = Vec::new();
            for f in &files {
                let w = sherpa_onnx::Wave::read(&f.to_string_lossy())
                    .ok_or_else(|| anyhow!("讀不到 wav:{}", f.display()))?;
                let mut s = w.samples().to_vec();
                if w.sample_rate() != 16000 {
                    let r = sherpa_onnx::LinearResampler::create(w.sample_rate(), 16000).ok_or_else(|| anyhow!("resampler"))?;
                    s = r.resample(&s, true);
                }
                let t1 = Instant::now();
                let e = id.embed(&s).ok_or_else(|| anyhow!("embed 失敗"))?;
                let who = id.identify(&s, Instant::now());
                println!(
                    "{}: {} ms, dim {}, 分群 → {:?}",
                    f.file_name().unwrap_or_default().to_string_lossy(),
                    t1.elapsed().as_millis(),
                    e.len(),
                    who
                );
                embs.push((f.file_name().unwrap_or_default().to_string_lossy().to_string(), e));
            }
            println!("\n餘弦相似度矩陣:");
            print!("{:>12}", "");
            for (n, _) in &embs {
                print!("{:>12}", n.chars().take(10).collect::<String>());
            }
            println!();
            for (n, a) in &embs {
                print!("{:>12}", n.chars().take(10).collect::<String>());
                for (_, b) in &embs {
                    print!("{:>12.3}", SpeakerIdentifier::cosine(a, b));
                }
                println!();
            }
            println!("已知說話者:{:?}", id.speakers());
        }
        Cmd::Serve { model, port } => {
            let before = gpu::nvidia_used_mib();
            let mut spec = LlamaServerSpec::new(paths::gguf_model(&model), port);
            spec.log_file = Some(paths::data_dir().join("llama-server.log"));
            let mut p = LlamaServerProcess::spawn(spec)?;
            let t = p.wait_ready_blocking(Duration::from_secs(120))?;
            let after = gpu::nvidia_used_mib();
            println!(
                "就緒 {:.1}s,顯示記憶體增量 {:?} MiB;Ctrl+C 結束",
                t.as_secs_f32(),
                after.zip(before).map(|(a, b)| a.saturating_sub(b))
            );
            tokio::signal::ctrl_c().await?;
            p.shutdown();
        }
    }
    Ok(())
}

/// `system`、`app:<pid>` 或 `app:<exe>`(取該程式的最上層程序)。
fn parse_audio_source(spec: &str) -> Result<AudioSource> {
    if spec == "system" {
        return Ok(AudioSource::System);
    }
    let arg = spec
        .strip_prefix("app:")
        .ok_or_else(|| anyhow!("來源要是 system、app:<pid> 或 app:<exe>:{spec}"))?;
    let table = source::snapshot_processes();
    let pid = match arg.parse::<u32>() {
        Ok(pid) => pid,
        Err(_) => source::rebind(arg, &table, &source::enumerate_sessions().unwrap_or_default(), std::process::id())
            .map(|(pid, _)| pid)
            .ok_or_else(|| anyhow!("找不到程式 {arg}"))?,
    };
    let info = table.get(&pid).ok_or_else(|| anyhow!("pid {pid} 不存在"))?;
    Ok(AudioSource::App { pid, exe: info.exe.clone(), start_time: info.start_time })
}

fn run_capture_probe(source: &str, seconds: u64) -> Result<()> {
    let target = parse_audio_source(source)?;
    let (tx, rx) = crossbeam_channel::bounded(4096);
    let stats = Arc::new(CaptureStats::default());
    let control = Arc::new(SourceControl::new(target.clone()));
    let cap = start_capture(CaptureConfig::default(), control.clone(), stats.clone(), tx)?;
    println!("探測 {target:?} {seconds} 秒(每秒一行:封包 / frame / 靜音封包 / 不連續 / 16 kHz 樣本 / 峰值)");
    let t0 = Instant::now();
    let mut prev = stats.snapshot();
    for sec in 1..=seconds {
        let deadline = t0 + Duration::from_secs(sec);
        let (mut peak, mut n) = (0f32, 0usize);
        while let Ok(c) = rx.recv_deadline(deadline) {
            n += c.samples.len();
            peak = c.samples.iter().fold(peak, |m, &x| m.max(x.abs()));
        }
        let s = stats.snapshot();
        let waiting = if control.waiting() { "  等待程式重開" } else { "" };
        println!(
            "{sec:>4}s{waiting} 封包 {:>4}  frame {:>6}  靜音 {:>4}  不連續 {:>3}  樣本 {:>6}({:>3.0}%)  峰值 {peak:.3}",
            s.packets - prev.packets,
            s.frames - prev.frames,
            s.silent_packets - prev.silent_packets,
            s.discontinuities - prev.discontinuities,
            n,
            n as f64 / 160.0,
        );
        prev = s;
    }
    let elapsed = t0.elapsed().as_secs_f64();
    cap.stop();
    let s = stats.snapshot();
    println!(
        "合計:封包 {}、靜音封包 {}、不連續 {}、session {}、樣本/實際時間 {:.3}",
        s.packets,
        s.silent_packets,
        s.discontinuities,
        s.sessions,
        s.samples_out as f64 / 16_000.0 / elapsed
    );
    Ok(())
}

fn run_vad(seconds: u64) -> Result<()> {
    let (tx, rx) = crossbeam_channel::bounded(256);
    let mut seg = Segmenter::new(&VadConfig::default())?;
    let _cap = start_loopback(CaptureConfig::default(), tx)?;
    println!("開始擷取 {seconds} 秒,請播放影片…");
    let t0 = Instant::now();
    let mut n = 0;
    let mut total_samples: u64 = 0;
    let mut last_speaking = false;
    while t0.elapsed() < Duration::from_secs(seconds) {
        let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
        total_samples += chunk.samples.len() as u64;
        let peak = chunk.samples.iter().fold(0f32, |m, &x| m.max(x.abs()));
        for s in seg.push(&chunk.samples) {
            n += 1;
            println!(
                "[{:6.1}s] 段 {n:3}:起點 {:.2}s,長 {} ms",
                t0.elapsed().as_secs_f32(),
                s.start_sample as f32 / 16000.0,
                s.duration_ms()
            );
        }
        let sp = seg.speaking();
        if sp != last_speaking {
            last_speaking = sp;
            if sp {
                print!("  …講話中(峰值 {peak:.3})\r");
                std::io::stdout().flush().ok();
            }
        }
    }
    for s in seg.flush() {
        n += 1;
        println!("[flush] 段 {n}:長 {} ms", s.duration_ms());
    }
    println!(
        "共 {n} 段;收到 {:.1} 秒音訊(預期 {seconds} 秒)",
        total_samples as f32 / 16000.0
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_pipeline(
    seconds: u64,
    lock: Option<Lang>,
    threads: i32,
    translator: Option<Arc<dyn Translator>>,
    out: Option<PathBuf>,
    record: Option<PathBuf>,
    normalize: bool,
    asr_engine: tingyi_core::pipeline::AsrEngine,
    record_all: Option<PathBuf>,
    refine: tingyi_core::refine::RefineConfig,
) -> Result<()> {
    let mut cfg = PipelineConfig::default();
    cfg.lock_lang = lock;
    cfg.record_dir = record;
    cfg.record_all = record_all;
    cfg.refine = refine;
    cfg.normalize_audio = normalize;
    cfg.asr_engine = asr_engine;
    cfg.asr.num_threads = threads;
    if lock.is_some() {
        cfg.allowed_langs = HashSet::new();
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = pipeline::start(cfg, translator, tx)?;
    println!("開始 {seconds} 秒,請播放影片…(Ctrl+C 提前結束)");

    let mut out_file = out
        .map(|p| std::fs::File::create(&p).with_context(|| p.display().to_string()))
        .transpose()?;
    let mut snaps: Vec<TimingSnapshot> = Vec::new();
    let mut asr_ms: Vec<u32> = Vec::new();
    let mut gpu_samples: Vec<u32> = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(deadline);
    let mut gpu_tick = tokio::time::interval(Duration::from_secs(10));
    let t0 = Instant::now();
    let mut current_delta = String::new();
    let mut recovered_chars: usize = 0;

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            _ = gpu_tick.tick() => {
                if let Some(m) = gpu::nvidia_used_mib() { gpu_samples.push(m); }
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                let ts = t0.elapsed().as_secs_f32();
                if let Some(f) = out_file.as_mut() {
                    let _ = writeln!(f, "{}", serde_json::json!({"t": ts, "ev": &ev}));
                }
                match &ev {
                    PipelineEvent::Status { message } => println!("[{ts:6.1}s] {message}"),
                    PipelineEvent::Segment { id, audio_ms, rms_dbfs, .. } => println!("[{ts:6.1}s] #{id} 段落 {audio_ms} ms({rms_dbfs:.1} dBFS)"),
                    PipelineEvent::TranscriptMerged { into, from, text, .. } => println!("[{ts:6.1}s] #{from} 併入 #{into} → {text}"),
                    PipelineEvent::SourceRevised { id, text } => println!("[{ts:6.1}s] #{id} 修原文 → {text}"),
                    PipelineEvent::TranslationRevised { id, text } => println!("[{ts:6.1}s] #{id} 修譯文 → {text}"),
                    PipelineEvent::Recovered { id, after, text, lang, start_ms, end_ms, .. } => {
                        recovered_chars += if lang.is_cjk() { text.chars().count() } else { text.split_whitespace().count() };
                        println!("[{ts:6.1}s] #{id} 補回({:.1}–{:.1}s,接在 #{after:?} 後)[{}] {text}", *start_ms as f32 / 1000.0, *end_ms as f32 / 1000.0, lang.code());
                    }
                    PipelineEvent::Partial { text, .. } => { if !text.is_empty() { println!("[{ts:6.1}s]    預覽:{text}"); } }
                    PipelineEvent::Segmentation { min_silence_ms, max_speech_ms, reason } => println!("[{ts:6.1}s] 斷句參數:靜音 {min_silence_ms} ms、上限 {max_speech_ms} ms({reason})"),
                    PipelineEvent::Speakers { names } => println!("[{ts:6.1}s] 說話者:{names:?}"),
                    PipelineEvent::SpeakerMerged { from, into } => println!("[{ts:6.1}s] 說話者合併:{from} → {into}"),
                    PipelineEvent::Transcript { id, text, lang, asr_ms: a, will_translate, speaker, .. } => {
                        asr_ms.push(*a);
                        println!("[{ts:6.1}s] #{id} 辨識 {a} ms [{}]{}{} {text}", lang.code(), if *will_translate { "" } else { "(不翻)" },
                            speaker.as_ref().map(|s| format!(" 〈{s}〉")).unwrap_or_default());
                        current_delta.clear();
                    }
                    PipelineEvent::TranslationDelta { .. } => {}
                    PipelineEvent::TranslationDone { id, text, timing, .. } => {
                        snaps.push(timing.clone());
                        println!(
                            "[{ts:6.1}s] #{id} 譯文 首字 {:?} ms / 完成 {:?} ms / 端到端首字 {:?} ms → {text}",
                            timing.mt_first_ms, timing.mt_total_ms, timing.e2e_first_ms
                        );
                    }
                    PipelineEvent::TranslationFailed { id, error } => println!("[{ts:6.1}s] #{id} 翻譯失敗:{error}"),
                    PipelineEvent::Error { message } => println!("[{ts:6.1}s] 錯誤:{message}"),
                }
            }
        }
    }
    let stats_snapshot = handle.stats().snapshot();
    handle.stop().await;

    println!("\n===== 統計 =====");
    let ps = stats_snapshot;
    println!(
        "管線:段落 {} · 辨識均值 {} ms · 預覽 {} 次均值 {} ms · 說話者均值 {} ms · 譯文首字均值 {} ms · 合併 {} · 音訊積壓峰值 {}",
        ps.segments, ps.asr_avg_ms, ps.preview_count, ps.preview_avg_ms, ps.speaker_avg_ms, ps.mt_first_avg_ms, ps.merges, ps.audio_backlog_max
    );
    if ps.refine_count > 0 || ps.recovered > 0 {
        println!(
            "回修:視窗 {} · 均值 {} ms · 修原文 {} 句 · 補回 {} 句 / {} 字 · 重翻 {} 句",
            ps.refine_count, ps.refine_avg_ms, ps.revised_sources, ps.recovered, recovered_chars, ps.refine_mt_done
        );
    }
    let asr = tingyi_core::metrics::Stats::from_samples(asr_ms);
    println!("辨識 n={} p50={} p95={} max={} ms", asr.n, asr.p50, asr.p95, asr.max);
    if !snaps.is_empty() {
        let s = Summary::from_snapshots(&snaps);
        println!("翻譯首字(辨識後) n={} p50={} p95={} ms", s.mt_first.n, s.mt_first.p50, s.mt_first.p95);
        println!("翻譯完成(辨識後) n={} p50={} p95={} ms", s.mt_total.n, s.mt_total.p50, s.mt_total.p95);
        println!("端到端首字(VAD 句尾起) n={} p50={} p95={} max={} ms", s.e2e_first.n, s.e2e_first.p50, s.e2e_first.p95, s.e2e_first.max);
        println!("端到端完成 n={} p50={} p95={} ms", s.e2e_done.n, s.e2e_done.p50, s.e2e_done.p95);
        if let Some(f) = out_file.as_mut() {
            let _ = writeln!(f, "{}", serde_json::json!({"summary": s, "asr": asr}));
        }
    }
    if !gpu_samples.is_empty() {
        println!(
            "顯示記憶體(整卡已用)取樣:min {} / max {} MiB",
            gpu_samples.iter().min().unwrap(),
            gpu_samples.iter().max().unwrap()
        );
    }
    Ok(())
}

async fn run_mt(
    sentences: PathBuf,
    models: Vec<String>,
    port: u16,
    context_n: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let content = std::fs::read_to_string(&sentences)?;
    let items: Vec<(Lang, String)> = content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (lang, text) = l.split_once('\t').unwrap_or(("en", l));
            Ok((parse_lang(lang.trim())?, text.trim().to_string()))
        })
        .collect::<Result<_>>()?;
    println!("{} 句,模型:{:?}", items.len(), models);
    let mut out_file = out
        .map(|p| std::fs::File::create(&p).with_context(|| p.display().to_string()))
        .transpose()?;

    for model in &models {
        let before = gpu::nvidia_used_mib();
        let mut spec = LlamaServerSpec::new(paths::gguf_model(model), port);
        spec.log_file = Some(paths::data_dir().join(format!("llama-server-{model}.log")));
        let mut p = LlamaServerProcess::spawn(spec)?;
        let load = p.wait_ready_blocking(Duration::from_secs(180))?;
        let after = gpu::nvidia_used_mib();
        let vram = after.zip(before).map(|(a, b)| a.saturating_sub(b));
        println!("\n===== {model} =====\n載入 {:.1}s,顯示記憶體增量 {vram:?} MiB", load.as_secs_f32());

        let tr = LlamaServerTranslator::new(LlamaServerConfig::for_model(
            format!("http://127.0.0.1:{port}"),
            model,
        ));
        let mut noop = |_: &str| {};
        let _ = tr
            .translate(
                TranslateRequest {
                    text: "Hello.".into(),
                    src_lang: Lang::En,
                    ..Default::default()
                },
                &mut noop,
            )
            .await;

        let mut ctx: Vec<(String, String)> = Vec::new();
        let mut firsts = Vec::new();
        let mut totals = Vec::new();
        for (i, (lang, text)) in items.iter().enumerate() {
            let req = TranslateRequest {
                text: text.clone(),
                src_lang: *lang,
                context: ctx.iter().rev().take(context_n).rev().cloned().collect(),
                glossary: vec![],
                ..Default::default()
            };
            let mut noop = |_: &str| {};
            match tr.translate(req, &mut noop).await {
                Ok(r) => {
                    let txt = tingyi_core::postprocess::to_taiwan_terms(&r.text);
                    println!(
                        "{:2}. [{}] 首字 {:>4?} ms 完成 {:>5} ms | {text}\n    → {txt}",
                        i + 1,
                        lang.code(),
                        r.first_token_ms.unwrap_or(0),
                        r.total_ms
                    );
                    if let Some(f) = out_file.as_mut() {
                        let _ = writeln!(
                            f,
                            "{}",
                            serde_json::json!({"model": model, "i": i, "lang": lang.code(), "src": text, "mt": txt,
                                "first_ms": r.first_token_ms, "total_ms": r.total_ms,
                                "prompt_tokens": r.prompt_tokens, "completion_tokens": r.completion_tokens})
                        );
                    }
                    firsts.extend(r.first_token_ms);
                    totals.push(r.total_ms);
                    ctx.push((text.clone(), txt));
                }
                Err(e) => println!("{:2}. 失敗:{e:#}", i + 1),
            }
        }
        let f = tingyi_core::metrics::Stats::from_samples(firsts);
        let t = tingyi_core::metrics::Stats::from_samples(totals);
        println!(
            "{model}:首字 p50={} p95={} ms;完成 p50={} p95={} ms;顯示記憶體增量 {vram:?} MiB",
            f.p50, f.p95, t.p50, t.p95
        );
        if let Some(fo) = out_file.as_mut() {
            let _ = writeln!(fo, "{}", serde_json::json!({"model": model, "summary": {"first": f, "total": t, "vram_mib": vram, "load_s": load.as_secs_f32()}}));
        }
        println!("關閉:{}", if p.shutdown() { "已結束" } else { "未確認" });
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(())
}
