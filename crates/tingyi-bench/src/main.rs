//! tingyi-bench:技術驗證週的量測 CLI。
//!
//! 子命令:
//! - `devices`           列出預設輸出裝置
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
use tingyi_core::audio::capture::{default_render_device_name, start_loopback, CaptureConfig};
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
}

/// 建立正式辨識引擎;whisper 時順便啟動 whisper-server(回傳的 process 要活到辨識結束)。
fn make_engine(
    engine: &str,
    threads: i32,
    lock: Option<Lang>,
    port: u16,
) -> Result<(Box<dyn tingyi_core::asr::Transcriber>, Option<tingyi_core::llama::WhisperServerProcess>)> {
    use tingyi_core::asr::{SenseVoice, SenseVoiceConfig, WhisperServer, WhisperServerConfig};
    use tingyi_core::llama::{WhisperServerProcess, WhisperServerSpec};
    match engine {
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
            if let Some((n, t)) = gpu::nvidia_info() {
                println!("GPU:{n} {t} MiB,目前已用 {:?} MiB", gpu::nvidia_used_mib());
            }
        }
        Cmd::Vad { seconds } => run_vad(seconds)?,
        Cmd::Asr {
            seconds,
            lang,
            threads,
            out,
        } => {
            let lock = lang.as_deref().map(parse_lang).transpose()?;
            run_pipeline(seconds, lock, threads, None, out, None, true, tingyi_core::pipeline::AsrEngine::SenseVoice).await?;
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
        } => {
            let lock = lang.as_deref().map(parse_lang).transpose()?;
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
            let translator: Arc<dyn Translator> = Arc::new(LlamaServerTranslator::new(LlamaServerConfig {
                base_url: format!("http://127.0.0.1:{port}"),
                ..Default::default()
            }));
            // 熱身一句,讓固定前綴進快取。
            let mut noop = |_: &str| {};
            let _ = translator
                .translate(
                    TranslateRequest {
                        text: "Hello.".into(),
                        src_lang: Lang::En,
                        ..Default::default()
                    },
                    &mut noop,
                )
                .await;
            run_pipeline(seconds, lock, 4, Some(translator), out, record, !no_normalize, asr_engine).await?;
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
) -> Result<()> {
    let mut cfg = PipelineConfig::default();
    cfg.lock_lang = lock;
    cfg.record_dir = record;
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
                    PipelineEvent::TranscriptMerged { into, from, text } => println!("[{ts:6.1}s] #{from} 併入 #{into} → {text}"),
                    PipelineEvent::Partial { text, .. } => { if !text.is_empty() { println!("[{ts:6.1}s]    預覽:{text}"); } }
                    PipelineEvent::Segmentation { min_silence_ms, max_speech_ms, reason } => println!("[{ts:6.1}s] 斷句參數:靜音 {min_silence_ms} ms、上限 {max_speech_ms} ms({reason})"),
                    PipelineEvent::Speakers { names } => println!("[{ts:6.1}s] 說話者:{names:?}"),
                    PipelineEvent::Transcript { id, text, lang, asr_ms: a, will_translate, speaker } => {
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
    handle.stop().await;

    println!("\n===== 統計 =====");
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

        let tr = LlamaServerTranslator::new(LlamaServerConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        });
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
