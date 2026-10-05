//! 全域狀態:目前的 llama-server 子程序與管線句柄。

use std::sync::Mutex;
use tingyi_core::llama::LlamaServerProcess;
use tingyi_core::pipeline::PipelineHandle;

#[derive(Default)]
pub struct AppState {
    pub server: Mutex<Option<LlamaServerProcess>>,
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
    }
}
