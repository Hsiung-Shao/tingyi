//! 系統資源取樣:整機 CPU / 記憶體、本程序與指定子程序(llama-server、whisper-server)的 CPU / RSS、
//! NVIDIA GPU 使用率與顯示記憶體。背景 thread 每 `interval` 取樣一次,保留最近 N 筆。

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{CpuRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::gpu::{nvidia_sample, GpuSample};

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: f32,
    pub rss_mb: u32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SysSample {
    /// Unix 毫秒。
    pub ts_ms: u64,
    pub cpu_pct: f32,
    pub mem_used_mb: u32,
    pub mem_total_mb: u32,
    pub gpu: Option<GpuSample>,
    pub procs: Vec<ProcSample>,
}

/// 要追蹤的子程序(名字, pid)。
pub type Watched = Arc<Mutex<Vec<(String, u32)>>>;

pub struct Sampler {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    history: Arc<Mutex<VecDeque<SysSample>>>,
    watched: Watched,
}

impl Sampler {
    /// 啟動背景取樣。`keep` 為保留筆數(2 s × 150 = 5 分鐘)。
    pub fn start(interval: Duration, keep: usize, watched: Watched) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let history: Arc<Mutex<VecDeque<SysSample>>> = Arc::new(Mutex::new(VecDeque::with_capacity(keep)));
        let (stop2, hist2, watched2) = (stop.clone(), history.clone(), watched.clone());
        let join = std::thread::Builder::new()
            .name("tingyi-sysmon".into())
            .spawn(move || {
                let mut sys = System::new();
                let self_pid = Pid::from_u32(std::process::id());
                // System::new() 的 CPU 清單是空的,refresh_cpu_specifics 只更新既有項目,
                // 所以第一次要用 refresh_cpu_list 建清單;這一輪同時是 CPU % 的基準。
                // 整機 CPU 用各核平均(global_cpu_usage 在 Windows 上第一輪常回 0)。
                sys.refresh_cpu_list(CpuRefreshKind::nothing().with_cpu_usage());
                sys.refresh_memory();
                let mut last = Instant::now();
                while !stop2.load(Ordering::Relaxed) {
                    let elapsed = last.elapsed();
                    if elapsed < interval {
                        std::thread::sleep((interval - elapsed).min(Duration::from_millis(250)));
                        continue;
                    }
                    last = Instant::now();
                    let mut pids: Vec<Pid> = vec![self_pid];
                    let watched_now: Vec<(String, u32)> = watched2.lock().unwrap().clone();
                    pids.extend(watched_now.iter().map(|(_, p)| Pid::from_u32(*p)));
                    sys.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage());
                    sys.refresh_memory();
                    sys.refresh_processes_specifics(
                        ProcessesToUpdate::Some(&pids),
                        true,
                        ProcessRefreshKind::nothing().with_cpu().with_memory(),
                    );
                    let mut procs = Vec::new();
                    let mut push = |name: &str, pid: Pid| {
                        if let Some(p) = sys.process(pid) {
                            procs.push(ProcSample {
                                pid: pid.as_u32(),
                                name: name.to_string(),
                                cpu_pct: p.cpu_usage(),
                                rss_mb: (p.memory() / 1_000_000) as u32,
                            });
                        }
                    };
                    push("tingyi", self_pid);
                    for (n, p) in &watched_now {
                        push(n, Pid::from_u32(*p));
                    }
                    let sample = SysSample {
                        ts_ms: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0),
                        cpu_pct: {
                            let cpus = sys.cpus();
                            if cpus.is_empty() {
                                sys.global_cpu_usage()
                            } else {
                                cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
                            }
                        },
                        mem_used_mb: (sys.used_memory() / 1_000_000) as u32,
                        mem_total_mb: (sys.total_memory() / 1_000_000) as u32,
                        gpu: nvidia_sample(),
                        procs,
                    };
                    let mut h = hist2.lock().unwrap();
                    if h.len() >= keep {
                        h.pop_front();
                    }
                    h.push_back(sample);
                }
            })
            .expect("spawn sysmon");
        Self {
            stop,
            join: Some(join),
            history,
            watched,
        }
    }

    pub fn latest(&self) -> Option<SysSample> {
        self.history.lock().unwrap().back().cloned()
    }

    pub fn history(&self) -> Vec<SysSample> {
        self.history.lock().unwrap().iter().cloned().collect()
    }

    pub fn watched(&self) -> Watched {
        self.watched.clone()
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
