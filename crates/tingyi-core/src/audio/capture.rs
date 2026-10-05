//! WASAPI loopback 擷取預設輸出裝置,轉成 16 kHz 單聲道 f32 後送進 channel。
//!
//! 設計:
//! - 專屬 std thread(COM MTA),事件驯動 + 逾時輪詢(loopback 在無播放時事件可能不觸發)。
//! - 使用裝置 mix format 的取樣率與聲道數,請求 32-bit float 並開 autoconvert。
//! - 任何 WASAPI 錯誤(裝置拔除、預設裝置切換)→ 記錄後 500 ms 重新建立 session。

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use super::{bytes_to_f32, downmix_to_mono, TARGET_SAMPLE_RATE};

/// 擷取設定。
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// 每次送出的最小樣本數(16 kHz)。預設 480 = 30 ms。
    pub min_chunk_samples: usize,
    /// 事件等待逾時(毫秒);逾時後仍嘗試讀取,避免 loopback 無事件卡死。
    pub event_timeout_ms: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            min_chunk_samples: 480,
            event_timeout_ms: 100,
        }
    }
}

/// 送出的音訊塊:16 kHz 單聲道。
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
}

/// 擷取執行緒句柄;drop 或呼叫 `stop()` 結束。
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl CaptureHandle {
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

/// 啟動 loopback 擷取執行緒。
pub fn start_loopback(cfg: CaptureConfig, tx: Sender<AudioChunk>) -> Result<CaptureHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let join = std::thread::Builder::new()
        .name("tingyi-capture".into())
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            while !stop2.load(Ordering::SeqCst) {
                match run_session(&cfg, &tx, &stop2) {
                    Ok(()) => break, // stop 要求
                    Err(e) => {
                        tracing::warn!("WASAPI loopback 中斷,500 ms 後重建:{e:#}");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
            tracing::info!("capture thread 結束");
        })
        .context("spawn capture thread")?;
    Ok(CaptureHandle {
        stop,
        join: Some(join),
    })
}

fn run_session(cfg: &CaptureConfig, tx: &Sender<AudioChunk>, stop: &AtomicBool) -> Result<()> {
    let enumerator = DeviceEnumerator::new().map_err(|e| anyhow!("DeviceEnumerator: {e}"))?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| anyhow!("default render device: {e}"))?;
    let name = device.get_friendlyname().unwrap_or_default();
    let mut client = device
        .get_iaudioclient()
        .map_err(|e| anyhow!("get_iaudioclient: {e}"))?;

    let mix = client
        .get_mixformat()
        .map_err(|e| anyhow!("get_mixformat: {e}"))?;
    let rate = mix.get_samplespersec();
    let channels = mix.get_nchannels() as usize;
    let format = WaveFormat::new(32, 32, &SampleType::Float, rate as usize, channels, None);
    let blockalign = format.get_blockalign() as usize;

    let (def_period, _min_period) = client
        .get_device_period()
        .map_err(|e| anyhow!("get_device_period: {e}"))?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: def_period,
    };
    // Render 裝置 + Capture 方向 = loopback。
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| anyhow!("initialize_client(loopback): {e}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|e| anyhow!("set_get_eventhandle: {e}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|e| anyhow!("get_audiocaptureclient: {e}"))?;

    let resampler = sherpa_onnx::LinearResampler::create(rate as i32, TARGET_SAMPLE_RATE)
        .ok_or_else(|| anyhow!("LinearResampler::create({rate} → {TARGET_SAMPLE_RATE})"))?;

    client
        .start_stream()
        .map_err(|e| anyhow!("start_stream: {e}"))?;
    tracing::info!("loopback 開始:{name} {rate} Hz {channels} ch → {TARGET_SAMPLE_RATE} Hz mono");

    let mut raw: VecDeque<u8> = VecDeque::with_capacity(blockalign * 4096);
    let mut pending: Vec<f32> = Vec::with_capacity(cfg.min_chunk_samples * 2);

    while !stop.load(Ordering::SeqCst) {
        // 逾時不是錯誤:loopback 在靜音時可能沒有事件,照樣嘗試讀。
        let _ = event.wait_for_event(cfg.event_timeout_ms);
        capture
            .read_from_device_to_deque(&mut raw)
            .map_err(|e| anyhow!("read_from_device_to_deque: {e}"))?;

        let usable = raw.len() - (raw.len() % blockalign);
        if usable == 0 {
            continue;
        }
        let bytes: Vec<u8> = raw.drain(..usable).collect();
        let interleaved = bytes_to_f32(&bytes);
        let mono = downmix_to_mono(&interleaved, channels);
        let out = resampler.resample(&mono, false);
        pending.extend_from_slice(&out);

        if pending.len() >= cfg.min_chunk_samples {
            let samples = std::mem::take(&mut pending);
            if tx.send(AudioChunk { samples }).is_err() {
                tracing::info!("接收端已關閉,停止擷取");
                break;
            }
        }
    }
    let _ = client.stop_stream();
    Ok(())
}
