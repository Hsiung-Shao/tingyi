//! 全域狀態:共用翻譯伺服器、whisper-server 子程序與管線句柄。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tingyi_core::audio::source::SourceControl;
use tingyi_core::idle::IdleControl;
use tingyi_core::model_download::DownloadRegistry;
use tingyi_core::llama::WhisperServerProcess;
use tingyi_core::pipeline::PipelineHandle;
use tingyi_core::sysmon::{Sampler, Watched};
use tingyi_core::translate_server::{self as ts, LlamaLauncher, SharedServer};
use tingyi_core::tuner::SegControl;
use tingyi_core::paths;

/// 共用翻譯伺服器(字幕與瀏覽器擴充);第一次用到時才建立。
pub type TranslateSlot = Arc<Mutex<Option<SharedServer>>>;

#[derive(Default)]
pub struct AppState {
    /// 系統取樣器(第一次 get_metrics 時才啟動);watched 列子程序 pid。
    pub sampler: Mutex<Option<Sampler>>,
    pub watched: Watched,
    pub translate: TranslateSlot,
    pub whisper: Mutex<Option<WhisperServerProcess>>,
    /// 斷句控制面;跨回合保留使用者的偏移與自動開關,start_session 時交給管線。
    pub seg_control: Arc<SegControl>,
    /// 音訊來源;跨回合保留,執行中切換由擷取執行緒即時套用。
    pub audio_source: Arc<SourceControl>,
    /// 懸浮字幕閒置隱藏秒數;跨回合保留,事件轉發端每次輪詢時讀。
    pub idle_control: Arc<IdleControl>,
    /// 進行中的專用模型下載(同一個模型同時只能一個)。
    pub downloads: Arc<DownloadRegistry>,
    pub pipeline: Mutex<Option<PipelineHandle>>,
    pub forwarder: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 瀏覽器擴充(網頁翻譯)的連線狀態。
    pub web: crate::web::WebLink,
    /// 目前回合是主視窗還是擴充開的;沒有回合為 None。
    pub owner: Mutex<Option<crate::listen::Owner>>,
    /// 擴充開回合前的音訊來源,停止後還原。
    pub listen_prev_source: Mutex<Option<tingyi_core::audio::source::AudioSource>>,
    /// 字幕事件(一行一則 JSON),給擴充的 named pipe 訂閱。
    pub live: LiveBus,
    /// OBS 瀏覽器來源(直播用網址)的事件緩衝與伺服器。
    pub obs: crate::obs::Obs,
    /// 有回合正在啟動(載模型要數秒):主視窗與擴充同時開始時只讓一個進去。
    pub starting: std::sync::atomic::AtomicBool,
    /// 擴充斷線後正在停止它開的回合。
    pub stopping: std::sync::atomic::AtomicBool,
}

pub struct LiveBus(pub tokio::sync::broadcast::Sender<String>);

impl Default for LiveBus {
    fn default() -> Self {
        Self(tokio::sync::broadcast::channel(256).0)
    }
}

/// 鎖住共用翻譯伺服器;第一次用到時決定 port 與 api key,整個程式執行期間不變。
/// 啟動或換模型會在鎖內等到就緒(最長數十秒),呼叫端要在 blocking 執行緒裡用。
pub fn lock_translate(slot: &TranslateSlot) -> anyhow::Result<MutexGuard<'_, Option<SharedServer>>> {
    let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        let job = match tingyi_core::job::Job::new() {
            Ok(j) => Some(Arc::new(j)),
            Err(e) => {
                tracing::warn!("建立 Job Object 失敗,llama-server 可能在程式被強制結束時殘留:{e}");
                None
            }
        };
        let launcher = LlamaLauncher {
            job,
            log_file: Some(paths::data_dir().join("llama-server.log")),
            ready_wait: Duration::from_secs(180),
        };
        *g = Some(SharedServer::new(paths::llama_server_exe(), ts::free_port()?, ts::new_api_key(), Box::new(launcher)));
    }
    Ok(g)
}

impl AppState {
    pub fn is_running(&self) -> bool {
        self.pipeline.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    /// 關閉時同步清理(視窗關閉事件沒有 async 環境)。
    pub fn shutdown_blocking(&self) {
        // 下載執行緒會在下一次讀取後停下,暫存檔保留供下次續傳
        self.downloads.cancel_all();
        if let Ok(mut p) = self.pipeline.lock() {
            // drop 即釋放擷取執行緒;worker 會隨 channel 斷線結束。
            p.take();
        }
        // 伺服器啟動中會在鎖內等就緒(最長 180 s),結束時不等它:
        // llama-server 在 Job Object 裡,本程式一結束就會被系統一起關掉
        match self.translate.try_lock() {
            Ok(mut g) => {
                // 一次放掉,不讓「字幕放手 → 切回網頁模型」在結束時重啟伺服器
                if let Some(s) = g.as_mut() {
                    s.release_all();
                }
            }
            Err(_) => tracing::info!("翻譯伺服器啟動中,交給 Job Object 隨程式結束"),
        }
        if let Ok(mut w) = self.whisper.lock() {
            if let Some(mut proc) = w.take() {
                proc.shutdown();
            }
        }
    }
}
