//! 全域狀態:目前的 llama-server 子程序與管線句柄。

use std::sync::{Arc, Mutex};
use tingyi_core::llama::{LlamaServerProcess, WhisperServerProcess};
use tingyi_core::pipeline::PipelineHandle;
use tingyi_core::tuner::SegControl;

#[derive(Default)]
pub struct AppState {
    pub server: Mutex<Option<LlamaServerProcess>>,
    pub whisper: Mutex<Option<WhisperServerProcess>>,
    /// 斷句控制面;跨回合保留使用者的偏移與自動開關,start_session 時交給管線。
    pub seg_control: Arc<SegControl>,
    pub pipeline: Mutex<Option<PipelineHandle>>,
    pub forwarder: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AppState {
    pub fn is_running(&self) -> bool {
        self.pipeline.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    /// 關閉時同步清理(視窗關閉事件沒有 async 環境)。
    pub fn shutdown_blocking(&self) {
        if let Ok(mut p) = self.pipeline.lock() {
            // drop 即釋放擷取執行緒;worker 會隨 channel 斷線結束。
            p.take();
        }
        if let Ok(mut s) = self.server.lock() {
            if let Some(mut proc) = s.take() {
                proc.shutdown();
            }
        }
        if let Ok(mut w) = self.whisper.lock() {
            if let Some(mut proc) = w.take() {
                proc.shutdown();
            }
        }
    }
}
