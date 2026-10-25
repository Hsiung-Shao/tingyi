//! WASAPI 擷取(系統 loopback 或指定程式),轉成 16 kHz 單聲道 f32 後送進 channel。
//!
//! 設計:
//! - 專屬 std thread(COM MTA),事件驅動 + 逾時輪詢(loopback 在無播放時事件可能不觸發)。
//! - 每次喚醒把待讀封包讀到空:事件是自動重設的,執行緒被延遲時多個封包只換到一次喚醒,
//!   只讀一包會越積越多,緩衝溢位後靜默掉音。
//! - 系統:用預設輸出裝置 mix format 的取樣率與聲道數;指定程式(process loopback)沒有
//!   mix format,固定要求 48 kHz 2ch float,由 autoconvert 轉換。
//! - 任何 WASAPI 錯誤(裝置拔除、預設裝置切換)→ 記錄後 500 ms 重新建立 session。
//! - 來源由 `SourceControl` 決定:世代號一變(使用者切換、程式重開重綁)就送出尾巴、立即重建;
//!   指定程式每秒檢查還活著沒,結束就依執行檔名重綁,找不到則等待並標記 waiting。

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode, WaveFormat,
};

use super::source::{AudioSource, ProcWatch, SourceControl};
use super::{bytes_to_f32, downmix_to_mono, TARGET_SAMPLE_RATE};

/// 擷取設定。
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// 每次送出的最小樣本數(16 kHz)。預設 480 = 30 ms。
    pub min_chunk_samples: usize,
    /// 事件等待逾時(毫秒);逾時後仍嘗試讀取,避免 loopback 無事件卡死。
    pub event_timeout_ms: u32,
    /// WASAPI 共享緩衝長度(毫秒)。原本用裝置週期(約 10 ms),執行緒稍被延遲就溢位。
    pub buffer_ms: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            min_chunk_samples: 480,
            event_timeout_ms: 100,
            buffer_ms: 200,
        }
    }
}

/// 一個 session 實際要開的對象。
#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureTarget {
    /// 預設輸出裝置的全部聲音。
    System,
    /// 指定程式(含子程序)發出的聲音。
    Process { pid: u32 },
}

/// 擷取計數(診斷用,原子更新)。
#[derive(Debug, Default)]
pub struct CaptureStats {
    pub packets: AtomicU64,
    pub frames: AtomicU64,
    /// 帶 AUDCLNT_BUFFERFLAGS_SILENT 的封包數(內容當靜音處理)。
    pub silent_packets: AtomicU64,
    /// 帶 AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY 的封包數(緩衝溢位、掉資料)。
    pub discontinuities: AtomicU64,
    /// 已送出的 16 kHz 樣本數。
    pub samples_out: AtomicU64,
    /// 建立 session 的次數(重建或切換來源都會加一)。
    pub sessions: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CaptureStatsSnapshot {
    pub packets: u64,
    pub frames: u64,
    pub silent_packets: u64,
    pub discontinuities: u64,
    pub samples_out: u64,
    pub sessions: u64,
}

impl CaptureStats {
    pub fn snapshot(&self) -> CaptureStatsSnapshot {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        CaptureStatsSnapshot {
            packets: g(&self.packets),
            frames: g(&self.frames),
            silent_packets: g(&self.silent_packets),
            discontinuities: g(&self.discontinuities),
            samples_out: g(&self.samples_out),
            sessions: g(&self.sessions),
        }
    }
}

/// 送出的音訊塊:16 kHz 單聲道。
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    /// 來源世代號;變了代表來源已切換,下游在切換點插入靜音、回修不跨切換點。
    pub gen: u64,
}

/// 擷取執行緒句柄;drop 或呼叫 `stop()` 結束。
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    stats: Arc<CaptureStats>,
}

impl CaptureHandle {
    pub fn stats(&self) -> Arc<CaptureStats> {
        self.stats.clone()
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 列出目前預設輸出裝置名稱(診斷用)。
pub fn default_render_device_name() -> Result<String> {
    let _ = wasapi::initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(|e| anyhow!("DeviceEnumerator: {e}"))?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| anyhow!("default render device: {e}"))?;
    device
        .get_friendlyname()
        .map_err(|e| anyhow!("friendly name: {e}"))
}

/// 啟動預設輸出裝置的 loopback 擷取執行緒。
pub fn start_loopback(cfg: CaptureConfig, tx: Sender<AudioChunk>) -> Result<CaptureHandle> {
    start_capture(cfg, Arc::new(SourceControl::default()), Arc::new(CaptureStats::default()), tx)
}

/// 啟動擷取執行緒;來源隨 `control` 即時切換。
pub fn start_capture(
    cfg: CaptureConfig,
    control: Arc<SourceControl>,
    stats: Arc<CaptureStats>,
    tx: Sender<AudioChunk>,
) -> Result<CaptureHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let stats2 = stats.clone();
    let join = std::thread::Builder::new()
        .name("tingyi-capture".into())
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            capture_thread(&cfg, &control, &stats2, &tx, &stop2);
            tracing::info!("capture thread 結束");
        })
        .context("spawn capture thread")?;
    Ok(CaptureHandle {
        stop,
        join: Some(join),
        stats,
    })
}

/// pump 結束的原因。
enum PumpExit {
    /// stop 要求或接收端已關閉。
    Stop,
    /// 來源世代號變了。
    Switched,
    /// 指定程式已結束。
    TargetGone,
}

fn capture_thread(
    cfg: &CaptureConfig,
    control: &SourceControl,
    stats: &CaptureStats,
    tx: &Sender<AudioChunk>,
    stop: &AtomicBool,
) {
    capture_loop(cfg, control, stats, tx, stop);
    // 結束後不再等程式重開,別讓 waiting 殘留給前端
    control.set_waiting(false);
}

fn capture_loop(
    cfg: &CaptureConfig,
    control: &SourceControl,
    stats: &CaptureStats,
    tx: &Sender<AudioChunk>,
    stop: &AtomicBool,
) {
    let mut watch = ProcWatch::new();
    while !stop.load(Ordering::SeqCst) {
        let (source, gen) = control.get();
        let target = match &source {
            AudioSource::System => CaptureTarget::System,
            AudioSource::App { pid, exe, start_time } => {
                if !watch.alive(*pid, *start_time) {
                    match watch.rebind(exe) {
                        Some((pid, start_time)) => {
                            tracing::info!("{exe} 重新開啟,改綁 pid {pid}");
                            // 改綁期間使用者若已改選(世代變了)就不寫,下一輪重讀
                            let new = AudioSource::App { pid, exe: exe.clone(), start_time };
                            if control.set_if_gen(gen, new).is_some() {
                                control.set_waiting(false);
                            }
                        }
                        None => {
                            control.set_waiting(true);
                            sleep_unless_changed(Duration::from_secs(1), control, gen, stop);
                        }
                    }
                    continue;
                }
                CaptureTarget::Process { pid: *pid }
            }
        };
        control.set_waiting(false);
        let r = open_session(cfg, &target).and_then(|s| {
            stats.sessions.fetch_add(1, Ordering::Relaxed);
            pump(&s, cfg, tx, stop, stats, control, gen, &source, &mut watch)
        });
        match r {
            Ok(PumpExit::Stop) => break,
            Ok(PumpExit::Switched) => tracing::info!("音訊來源切換"),
            Ok(PumpExit::TargetGone) => tracing::info!("指定程式已結束"),
            Err(e) => {
                tracing::warn!("WASAPI 擷取中斷,500 ms 後重建:{e:#}");
                sleep_unless_changed(Duration::from_millis(500), control, gen, stop);
            }
        }
    }
}

/// 睡 `dur`,但 stop 或來源切換時提早醒來。
fn sleep_unless_changed(dur: Duration, control: &SourceControl, gen: u64, stop: &AtomicBool) {
    let t0 = Instant::now();
    while t0.elapsed() < dur && !stop.load(Ordering::SeqCst) && control.gen() == gen {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 一個已初始化、尚未開始的擷取 session。
struct Session {
    client: AudioClient,
    capture: AudioCaptureClient,
    event: Handle,
    rate: u32,
    channels: usize,
    blockalign: usize,
    label: String,
}

fn open_session(cfg: &CaptureConfig, target: &CaptureTarget) -> Result<Session> {
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: cfg.buffer_ms as i64 * 10_000,
    };
    let (mut client, rate, channels, label) = match target {
        CaptureTarget::System => {
            let enumerator = DeviceEnumerator::new().map_err(|e| anyhow!("DeviceEnumerator: {e}"))?;
            let device = enumerator
                .get_default_device(&Direction::Render)
                .map_err(|e| anyhow!("default render device: {e}"))?;
            let name = device.get_friendlyname().unwrap_or_default();
            let client = device
                .get_iaudioclient()
                .map_err(|e| anyhow!("get_iaudioclient: {e}"))?;
            let mix = client
                .get_mixformat()
                .map_err(|e| anyhow!("get_mixformat: {e}"))?;
            (client, mix.get_samplespersec(), mix.get_nchannels() as usize, name)
        }
        CaptureTarget::Process { pid } => {
            // include_tree:瀏覽器由子程序發聲,要連同整棵程序樹一起擷取。
            let client = AudioClient::new_application_loopback_client(*pid, true)
                .map_err(|e| anyhow!("process loopback(pid {pid}): {e}"))?;
            (client, 48_000, 2, format!("程式 pid {pid}"))
        }
    };
    let format = WaveFormat::new(32, 32, &SampleType::Float, rate as usize, channels, None);
    let blockalign = format.get_blockalign() as usize;
    // Render 裝置 + Capture 方向 = loopback;process loopback client 也用 Capture 方向。
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| anyhow!("initialize_client: {e}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|e| anyhow!("set_get_eventhandle: {e}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|e| anyhow!("get_audiocaptureclient: {e}"))?;
    Ok(Session {
        client,
        capture,
        event,
        rate,
        channels,
        blockalign,
        label,
    })
}

/// 開始串流並持續讀取,直到 stop、來源切換、指定程式結束(Ok)或 WASAPI 出錯(Err)。
#[allow(clippy::too_many_arguments)]
fn pump(
    s: &Session,
    cfg: &CaptureConfig,
    tx: &Sender<AudioChunk>,
    stop: &AtomicBool,
    stats: &CaptureStats,
    control: &SourceControl,
    gen: u64,
    source: &AudioSource,
    watch: &mut ProcWatch,
) -> Result<PumpExit> {
    let resampler = sherpa_onnx::LinearResampler::create(s.rate as i32, TARGET_SAMPLE_RATE)
        .ok_or_else(|| anyhow!("LinearResampler::create({} → {TARGET_SAMPLE_RATE})", s.rate))?;
    s.client
        .start_stream()
        .map_err(|e| anyhow!("start_stream: {e}"))?;
    tracing::info!(
        "擷取開始:{} {} Hz {} ch → {TARGET_SAMPLE_RATE} Hz mono",
        s.label,
        s.rate,
        s.channels
    );

    let mut raw: VecDeque<u8> = VecDeque::with_capacity(s.blockalign * 4096);
    let mut first_packet = true;
    let mut pending: Vec<f32> = Vec::with_capacity(cfg.min_chunk_samples * 2);
    let send = |samples: Vec<f32>| -> bool {
        stats.samples_out.fetch_add(samples.len() as u64, Ordering::Relaxed);
        tx.send(AudioChunk { samples, gen }).is_ok()
    };
    let mut last_alive_check = Instant::now();

    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(PumpExit::Stop);
        }
        if control.gen() != gen {
            break Ok(PumpExit::Switched);
        }
        // 程式結束時 process loopback 不報錯、只送靜音(實測),要自己輪詢。
        if let AudioSource::App { pid, start_time, .. } = source {
            if last_alive_check.elapsed() >= Duration::from_secs(1) {
                last_alive_check = Instant::now();
                if !watch.alive(*pid, *start_time) {
                    break Ok(PumpExit::TargetGone);
                }
            }
        }
        // 逾時不是錯誤:loopback 在靜音時可能沒有事件,照樣嘗試讀。
        let _ = s.event.wait_for_event(cfg.event_timeout_ms);
        if let Err(e) = drain_packets(s, &mut raw, stats, &mut first_packet) {
            break Err(e);
        }

        let usable = raw.len() - (raw.len() % s.blockalign);
        if usable == 0 {
            continue;
        }
        let bytes: Vec<u8> = raw.drain(..usable).collect();
        let interleaved = bytes_to_f32(&bytes);
        let mono = downmix_to_mono(&interleaved, s.channels);
        pending.extend_from_slice(&resampler.resample(&mono, false));

        if pending.len() >= cfg.min_chunk_samples && !send(std::mem::take(&mut pending)) {
            tracing::info!("接收端已關閉,停止擷取");
            break Ok(PumpExit::Stop);
        }
    };
    // 結束(含出錯重建)前把不到一塊的尾巴送出,避免每次重建掉幾十毫秒。
    if !pending.is_empty() {
        send(pending);
    }
    let _ = s.client.stop_stream();
    result
}

/// 把目前所有待讀封包讀進 `raw`;SILENT 封包的內容依規格當靜音。
/// 串流開始的第一個封包本來就帶 DATA_DISCONTINUITY(實測),不計入掉資料。
fn drain_packets(s: &Session, raw: &mut VecDeque<u8>, stats: &CaptureStats, first_packet: &mut bool) -> Result<()> {
    loop {
        let frames = s
            .capture
            .get_next_packet_size()
            .map_err(|e| anyhow!("get_next_packet_size: {e}"))?
            .unwrap_or(0);
        if frames == 0 {
            return Ok(());
        }
        let before = raw.len();
        let info = s
            .capture
            .read_from_device_to_deque(raw)
            .map_err(|e| anyhow!("read_from_device_to_deque: {e}"))?;
        stats.packets.fetch_add(1, Ordering::Relaxed);
        stats
            .frames
            .fetch_add(((raw.len() - before) / s.blockalign) as u64, Ordering::Relaxed);
        if info.flags.silent {
            stats.silent_packets.fetch_add(1, Ordering::Relaxed);
            raw.range_mut(before..).for_each(|b| *b = 0);
        }
        if info.flags.data_discontinuity && !*first_packet {
            stats.discontinuities.fetch_add(1, Ordering::Relaxed);
        }
        *first_packet = false;
    }
}
