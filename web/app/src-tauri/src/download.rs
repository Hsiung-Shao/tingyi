//! llama.cpp 與翻譯模型的下載:固定網址逐檔下載、續傳、SHA256 校驗、暫存後提交(改自 tingyi `model_download.rs`)。
//!
//! - 每檔先下載到 `<parent>/.download/<dir>/<name>.part`;寫完整檔校驗大小與 SHA256,通過才改名去掉
//!   `.part`(改名 = 已校驗的提交點,重啟後可直接信任)。
//! - 全部檔案提交後才放進正式目錄 `<parent>/<dir>`:目錄不存在就整個改名過去,已存在就逐檔移入,
//!   不刪使用者自己放的檔案。半途中斷時正式目錄不會出現半套。
//! - 同步實作,跑在 std 執行緒;取消在每次讀取後檢查。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 套件裡的一個檔案(大小與 SHA256 固定,換版本要一起改)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub name: String,
    /// 相對 `Package::base_url` 的路徑
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// 一個可下載的套件:安裝到 `<parent>/<dir_name>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub id: String,
    pub label: String,
    pub dir_name: String,
    pub base_url: String,
    pub files: Vec<FileSpec>,
    /// 安裝後把每個 .zip 解壓到套件目錄(檔案平放,不保留 zip 內的頂層資料夾)
    pub unzip: bool,
    /// 安裝完成的判斷依據:這個檔案存在於套件目錄(例如 llama-server.exe)
    pub ready_marker: String,
}

impl Package {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// `{base}/{path}`;每次都從這個原始網址開始(HF、GitHub 會 302 到會過期的 CDN 網址)
    pub fn url(&self, base: &str, file: &FileSpec) -> String {
        format!("{}/{}", base.trim_end_matches('/'), file.path)
    }
}

pub const LLAMA_BUILD: &str = "b11405";
pub const MODEL_FILE: &str = "Hy-MT2-1.8B-Q8_0.gguf";

/// llama.cpp 預編譯 Vulkan 版(NVIDIA/AMD/Intel 都能用,沒有 GPU 時退回 CPU);安裝到 `bin/llama`
pub fn llama_package() -> Package {
    let zip = format!("llama-{LLAMA_BUILD}-bin-win-vulkan-x64.zip");
    Package {
        id: "llama".into(),
        label: format!("llama.cpp {LLAMA_BUILD}(Vulkan)"),
        dir_name: "llama".into(),
        base_url: "https://github.com".into(),
        files: vec![FileSpec {
            path: format!("ggml-org/llama.cpp/releases/download/{LLAMA_BUILD}/{zip}"),
            name: zip,
            size: 33_308_372,
            sha256: "0f3764246cc4c365153dc33a24448921526d91bba5ee3195308fa692d2031884".into(),
        }],
        unzip: true,
        ready_marker: "llama-server.exe".into(),
    }
}

/// Hy-MT2 1.8B Q8_0(tencent/Hy-MT2-1.8B-GGUF 固定 commit,Apache-2.0);安裝到 `models/hy-mt2-1.8b`
pub fn model_package() -> Package {
    Package {
        id: "model".into(),
        label: "Hy-MT2 1.8B 翻譯模型".into(),
        dir_name: "hy-mt2-1.8b".into(),
        base_url: "https://huggingface.co".into(),
        files: vec![FileSpec {
            name: MODEL_FILE.into(),
            path: format!("tencent/Hy-MT2-1.8B-GGUF/resolve/a0c709d9fac510f2c807aa3af52872340dc37a4a/{MODEL_FILE}"),
            size: 1_908_528_192,
            sha256: "5c3fe0b1408a5ceb0143184ef247b11b579c525f4b02b060e6c851bb76fef1a4".into(),
        }],
        unzip: false,
        ready_marker: MODEL_FILE.into(),
    }
}

fn file_len(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// 套件已安裝:`<parent>/<dir_name>/<ready_marker>` 存在,且每個檔案大小正確
pub fn package_ready(pkg: &Package, parent: &Path) -> bool {
    let dir = parent.join(&pkg.dir_name);
    dir.join(&pkg.ready_marker).is_file() && pkg.files.iter().all(|f| file_len(&dir.join(&f.name)) == Some(f.size))
}

/// 把 zip 平放解壓到 dest(zip 內的子目錄照原樣建立,拒絕跳出 dest 的路徑)
pub fn extract_zip(zip_path: &Path, dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?).map_err(io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        // enclosed_name 拒絕絕對路徑與 `..`(zip slip)
        let Some(rel) = entry.enclosed_name() else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("zip 內有不安全的路徑:{}", entry.name())));
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create(&out)?)?;
    }
    Ok(())
}

/// 小於這個大小的檔案一律從頭下載(不值得續傳,HF 也可能不經 CDN 直接回應)。
pub const SMALL_FILE: u64 = 1 << 20;

/// 單一檔案該怎麼處理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePlan {
    /// 從頭下載(沒有暫存檔、暫存檔是空的,或小檔)。
    Fresh,
    /// 從這個位元組接續(送 `Range: bytes=n-`)。
    Resume(u64),
    /// 暫存檔已經完整:不發請求,直接校驗。
    VerifyOnly,
    /// 暫存檔比應有的大:刪掉重下。
    Restart,
    /// 已提交(去掉 .part 且大小正確):略過。
    Skip,
}

/// `part_len`:暫存檔長度;`committed_len`:暫存目錄裡已提交檔案的長度。
pub fn plan_file(part_len: Option<u64>, committed_len: Option<u64>, size: u64) -> FilePlan {
    if committed_len == Some(size) {
        return FilePlan::Skip;
    }
    match part_len {
        None | Some(0) => FilePlan::Fresh,
        Some(n) if n == size => FilePlan::VerifyOnly,
        Some(n) if n > size => FilePlan::Restart,
        Some(_) if size < SMALL_FILE => FilePlan::Fresh,
        Some(n) => FilePlan::Resume(n),
    }
}

/// 解析 `Content-Range: bytes S-E/T`。
pub fn parse_content_range(v: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = v.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// 收到回應後怎麼做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseAction {
    /// 206 且起點正確:接在暫存檔後面寫。
    Append,
    /// 200(伺服器忽略 Range):截斷暫存檔從頭寫。
    Truncate,
    /// 範圍對不上(206 起點或總長錯、416):刪掉暫存檔重下。
    Restart,
    /// 暫時性錯誤(429、5xx):稍後以 Range 重試。
    Retry,
    /// 不可重試的錯誤(404 等)。
    Fail(String),
}

/// `from`:這次請求的起點(0 = 沒送 Range);`size`:檔案應有大小。
pub fn classify_response(
    status: u16,
    content_range: Option<&str>,
    content_length: Option<u64>,
    from: u64,
    size: u64,
) -> ResponseAction {
    match status {
        206 => match content_range.and_then(parse_content_range) {
            Some((start, end, total)) if start == from && total == size && end + 1 == size => ResponseAction::Append,
            _ => ResponseAction::Restart,
        },
        200 => match content_length {
            Some(len) if len != size => ResponseAction::Fail(format!("HTTP 200 但長度 {len} 與預期 {size} 不符")),
            _ => ResponseAction::Truncate,
        },
        416 => ResponseAction::Restart,
        429 | 500..=599 => ResponseAction::Retry,
        other => ResponseAction::Fail(format!("HTTP {other}")),
    }
}

/// 下載階段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Downloading,
    Verifying,
    Done,
    Error,
    Cancelled,
}

/// 推給前端的進度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub id: String,
    pub phase: Phase,
    /// 已下載位元組(整個模型合計)。
    pub done: u64,
    pub total: u64,
    /// 最近的下載速度(位元組/秒)。
    pub bps: u64,
    pub message: Option<String>,
}

/// 進度節流:最多每 `PROGRESS_INTERVAL` 一次,或進度變化 ≥ 1% 時;完成一律發。
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub struct ProgressThrottle {
    total: u64,
    last: Option<(Instant, u64)>,
}

impl ProgressThrottle {
    pub fn new(total: u64) -> Self {
        Self { total, last: None }
    }

    /// 這次要不要發;回 true 時記住這次的時間與進度。
    pub fn should_emit(&mut self, now: Instant, done: u64) -> bool {
        let emit = match self.last {
            None => true,
            Some(_) if done >= self.total => true,
            Some((t, d)) => {
                now.saturating_duration_since(t) >= PROGRESS_INTERVAL || done.saturating_sub(d) >= (self.total / 100).max(1)
            }
        };
        if emit {
            self.last = Some((now, done));
        }
        emit
    }
}

/// 把 I/O 錯誤轉成使用者看得懂的訊息(磁碟不足、檔案被佔用),其餘原樣。
pub fn map_io_error(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(112 | 39) => format!("磁碟空間不足,請清出空間後按「繼續」({e})"),
        Some(5 | 32 | 33) => format!("檔案被其他程式佔用(可能是防毒掃描),請稍後重試({e})"),
        _ if e.kind() == io::ErrorKind::StorageFull => format!("磁碟空間不足,請清出空間後按「繼續」({e})"),
        _ => e.to_string(),
    }
}

/// 一個下載任務的控制與進度快照(前端重載後可查回)。
#[derive(Debug, Default)]
pub struct DownloadCtl {
    pub cancel: AtomicBool,
    pub done: AtomicU64,
    pub total: AtomicU64,
}

/// 進行中的下載;同一個模型同時只能有一個任務。
#[derive(Debug, Default)]
pub struct DownloadRegistry {
    active: Mutex<HashMap<String, Arc<DownloadCtl>>>,
}

/// 下載任務的登記;drop(含 panic 展開)時自動移除。
pub struct DownloadGuard {
    registry: Arc<DownloadRegistry>,
    id: String,
    ctl: Arc<DownloadCtl>,
}

impl DownloadRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<DownloadCtl>>> {
        // 持鎖期間 panic 也不能讓登記表永久失效
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 在同一把鎖內檢查並登記;該模型已在下載中回 None。
    pub fn try_begin(self: &Arc<Self>, id: &str) -> Option<DownloadGuard> {
        let mut active = self.lock();
        if active.contains_key(id) {
            return None;
        }
        let ctl = Arc::new(DownloadCtl::default());
        active.insert(id.to_string(), ctl.clone());
        Some(DownloadGuard { registry: self.clone(), id: id.to_string(), ctl })
    }

    /// 要求取消;沒有在下載回 false。
    pub fn cancel(&self, id: &str) -> bool {
        match self.lock().get(id) {
            Some(c) => {
                c.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn cancel_all(&self) {
        for c in self.lock().values() {
            c.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// 進行中任務的 (已下載, 總量)。
    pub fn snapshot(&self, id: &str) -> Option<(u64, u64)> {
        self.lock()
            .get(id)
            .map(|c| (c.done.load(Ordering::Relaxed), c.total.load(Ordering::Relaxed)))
    }
}

impl DownloadGuard {
    pub fn ctl(&self) -> Arc<DownloadCtl> {
        self.ctl.clone()
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        self.registry.lock().remove(&self.id);
    }
}

fn is_transient_lock(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// 改名;遇到防毒掃描等暫時鎖檔(OS 錯誤 5/32/33)時以遞增間隔重試,最長 `max_wait`。
pub fn retry_rename(from: &Path, to: &Path, max_wait: Duration) -> io::Result<()> {
    let t0 = Instant::now();
    let mut wait = Duration::from_millis(50);
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_lock(&e) && t0.elapsed() + wait <= max_wait => {
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(1));
            }
            Err(e) => return Err(e),
        }
    }
}

/// 改名的重試上限(防毒掃描通常幾百毫秒內放開)。
const RENAME_MAX_WAIT: Duration = Duration::from_secs(10);

/// 下載參數(測試可縮短逾時與重試間隔)。
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// 覆寫下載來源(測試用);None 用套件自己的 base_url
    pub base_url: Option<String>,
    pub connect_timeout: Duration,
    /// 單次讀取逾時;不設總逾時(大檔在慢網路要下很久)。
    pub read_timeout: Duration,
    /// 暫時性錯誤(斷線、429、5xx)自動重試次數。
    pub retries: u32,
    pub retry_backoff: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            base_url: None,
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(60),
            retries: 3,
            retry_backoff: Duration::from_secs(1),
        }
    }
}

/// 下載失敗的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    /// 大小或 SHA256 不符(暫存檔已刪,重試會從頭下載)。
    Verify { file: String },
    Http { status: u16, file: String },
    Network(String),
    Io(String),
    DiskFull { need: u64, free: u64 },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => write!(f, "已取消,下次按「繼續」會從中斷處接著下載"),
            DownloadError::Verify { file } => write!(f, "{file} 校驗失敗(內容與預期不符),已刪除暫存檔,請重試"),
            DownloadError::Http { status, file } => write!(f, "下載 {file} 失敗:伺服器回應 HTTP {status}"),
            DownloadError::Network(e) => write!(f, "無法連線下載來源:{e}"),
            DownloadError::Io(e) => write!(f, "{e}"),
            DownloadError::DiskFull { need, free } => {
                write!(f, "磁碟空間不足:需要約 {} MB,剩 {} MB", need / 1_000_000, free / 1_000_000)
            }
        }
    }
}

fn io_err(e: io::Error) -> DownloadError {
    DownloadError::Io(map_io_error(&e))
}

fn staging_dir(spec: &Package, models_dir: &Path) -> PathBuf {
    models_dir.join(".download").join(&spec.dir_name)
}

fn part_path(staging: &Path, f: &FileSpec) -> PathBuf {
    staging.join(format!("{}.part", f.name))
}

/// 已經有的位元組(已提交的檔案算全部,暫存檔算已寫入的部分)。
fn bytes_present(staging: &Path, f: &FileSpec) -> u64 {
    if file_len(&staging.join(&f.name)) == Some(f.size) {
        return f.size;
    }
    file_len(&part_path(staging, f)).filter(|n| *n <= f.size).unwrap_or(0)
}

/// 尚未下載的位元組數(已提交的檔案與暫存檔已有的部分不算)。
pub fn bytes_needed(spec: &Package, models_dir: &Path) -> u64 {
    let staging = staging_dir(spec, models_dir);
    spec.files.iter().map(|f| f.size - bytes_present(&staging, f)).sum()
}

/// 磁碟剩餘空間(位元組);查不到回 None。
#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut avail = 0u64;
    // SAFETY:wide 以 0 結尾;三個輸出指標都指向有效的 u64(後兩個允許為 null,這裡給有效位址)。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(avail)
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// 開始前檢查磁碟空間(剩餘未下載量 + 64 MB 餘裕)。
pub fn check_disk_space(spec: &Package, models_dir: &Path) -> Result<(), DownloadError> {
    fs::create_dir_all(models_dir).map_err(io_err)?;
    let need = bytes_needed(spec, models_dir) + 64 * 1_000_000;
    match free_space(models_dir) {
        Some(free) if free < need => Err(DownloadError::DiskFull { need, free }),
        _ => Ok(()),
    }
}

fn sha256_file(p: &Path) -> io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 下載中的累計狀態與節流後的進度回報。
struct Reporter<'a> {
    id: &'a str,
    total: u64,
    done: u64,
    throttle: ProgressThrottle,
    speed_mark: (Instant, u64),
    bps: u64,
    ctl: &'a DownloadCtl,
    on_progress: &'a mut dyn FnMut(Progress),
}

impl Reporter<'_> {
    fn emit(&mut self, phase: Phase, message: Option<String>) {
        (self.on_progress)(Progress { id: self.id.to_string(), phase, done: self.done, total: self.total, bps: self.bps, message });
    }

    fn set_done(&mut self, done: u64) {
        self.done = done;
        self.ctl.done.store(done, Ordering::Relaxed);
        let now = Instant::now();
        if self.throttle.should_emit(now, done) {
            // 速度取至少 1 秒的平均,單次讀取的快慢不會讓數字亂跳
            let (t, d) = self.speed_mark;
            let secs = now.saturating_duration_since(t).as_secs_f64();
            if secs >= 1.0 {
                self.bps = (done.saturating_sub(d) as f64 / secs) as u64;
                self.speed_mark = (now, done);
            }
            self.emit(Phase::Downloading, None);
        }
    }
}

fn build_agent(opts: &DownloadOptions) -> Result<ureq::Agent, DownloadError> {
    let tls = native_tls::TlsConnector::new().map_err(|e| DownloadError::Network(format!("TLS 初始化失敗:{e}")))?;
    Ok(ureq::AgentBuilder::new()
        .timeout_connect(opts.connect_timeout)
        .timeout_read(opts.read_timeout)
        .user_agent(concat!("agent-translator/", env!("CARGO_PKG_VERSION")))
        .tls_connector(Arc::new(tls))
        .build())
}

/// 把一個檔案下載到 `.part`(含續傳、自動重試);回傳時 `.part` 長度等於 `f.size`。
fn fetch_part(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    f: &FileSpec,
    opts: &DownloadOptions,
    rep: &mut Reporter,
) -> Result<(), DownloadError> {
    let base_done = rep.done - file_len(part).unwrap_or(0).min(f.size);
    let mut attempt = 0u32;
    let mut best = file_len(part).unwrap_or(0); // 暫存檔達過的最大長度:只有超過它才算真的有進度
    let ctl = rep.ctl;
    let retry = |attempt: &mut u32, err: DownloadError| -> Result<(), DownloadError> {
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        *attempt += 1;
        if *attempt > opts.retries {
            return Err(err);
        }
        // 退避等待切小段輪詢取消旗標,取消時不必等完
        let deadline = Instant::now() + opts.retry_backoff * *attempt;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if ctl.cancel.load(Ordering::SeqCst) {
                return Err(DownloadError::Cancelled);
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
        Ok(())
    };
    loop {
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let have = file_len(part).unwrap_or(0);
        if have == f.size {
            return Ok(());
        }
        let from = if f.size < SMALL_FILE { 0 } else { have };
        let mut req = agent.get(url);
        if from > 0 {
            req = req.set("Range", &format!("bytes={from}-"));
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                retry(&mut attempt, DownloadError::Network(t.to_string()))?;
                continue;
            }
        };
        let status = resp.status();
        let content_length = resp.header("Content-Length").and_then(|v| v.trim().parse::<u64>().ok());
        let mut file = match classify_response(status, resp.header("Content-Range"), content_length, from, f.size) {
            ResponseAction::Append => OpenOptions::new().create(true).append(true).open(part).map_err(io_err)?,
            ResponseAction::Truncate => File::create(part).map_err(io_err)?,
            ResponseAction::Restart => {
                let _ = fs::remove_file(part);
                retry(&mut attempt, DownloadError::Http { status, file: f.name.to_string() })?;
                continue;
            }
            ResponseAction::Retry => {
                retry(&mut attempt, DownloadError::Http { status, file: f.name.to_string() })?;
                continue;
            }
            ResponseAction::Fail(_) => return Err(DownloadError::Http { status, file: f.name.to_string() }),
        };
        let mut written = file.metadata().map_err(io_err)?.len();
        rep.set_done(base_done + written);
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let read_result: Result<(), io::Error> = loop {
            if rep.ctl.cancel.load(Ordering::SeqCst) {
                let _ = file.flush();
                return Err(DownloadError::Cancelled);
            }
            match reader.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if written + n as u64 > f.size {
                        break Err(io::Error::new(io::ErrorKind::InvalidData, "伺服器送出的內容比預期長"));
                    }
                    file.write_all(&buf[..n]).map_err(io_err)?;
                    written += n as u64;
                    rep.set_done(base_done + written);
                }
                Err(e) => break Err(e),
            }
        };
        file.sync_data().map_err(io_err)?;
        drop(file);
        if written > best {
            best = written;
            // 重試上限算的是「連續沒有進度」的次數,大檔在弱網路斷幾次仍能下完;
            // 伺服器多送內容(InvalidData)或從頭重下(淨進度為 0)都不算進度
            if !matches!(&read_result, Err(e) if e.kind() == io::ErrorKind::InvalidData) {
                attempt = 0;
            }
        }
        match read_result {
            Ok(()) if written == f.size => return Ok(()),
            Ok(()) => retry(&mut attempt, DownloadError::Network(format!("{} 連線提早結束", f.name)))?,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let _ = fs::remove_file(part);
                retry(&mut attempt, DownloadError::Network(e.to_string()))?;
            }
            Err(e) => retry(&mut attempt, DownloadError::Network(e.to_string()))?,
        }
    }
}

/// 把通過校驗的檔案放進正式目錄:目錄不存在就整個改名過去,否則逐檔移入(不刪使用者的檔案)。
fn install(spec: &Package, staging: &Path, models_dir: &Path) -> Result<(), DownloadError> {
    let final_dir = models_dir.join(&spec.dir_name);
    if !final_dir.exists() && retry_rename(staging, &final_dir, RENAME_MAX_WAIT).is_ok() {
        return Ok(());
    }
    fs::create_dir_all(&final_dir).map_err(io_err)?;
    for f in &spec.files {
        let src = staging.join(&f.name);
        if src.exists() {
            // Windows 的 rename 會取代既有檔案
            retry_rename(&src, &final_dir.join(&f.name), RENAME_MAX_WAIT).map_err(io_err)?;
        }
    }
    let _ = fs::remove_dir_all(staging);
    Ok(())
}

/// 下載整個套件到 `models_dir/<dir_name>`(models_dir 即上層目錄);同步執行。
pub fn download_package(
    spec: &Package,
    models_dir: &Path,
    opts: &DownloadOptions,
    ctl: &DownloadCtl,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    let staging = staging_dir(spec, models_dir);
    fs::create_dir_all(&staging).map_err(io_err)?;
    let total = spec.total_size();
    ctl.total.store(total, Ordering::Relaxed);
    let present: u64 = spec.files.iter().map(|f| bytes_present(&staging, f)).sum();
    let mut rep = Reporter {
        id: &spec.id,
        total,
        done: present,
        throttle: ProgressThrottle::new(total),
        speed_mark: (Instant::now(), present),
        bps: 0,
        ctl,
        on_progress,
    };
    rep.set_done(present);
    let mut agent: Option<ureq::Agent> = None;
    for f in &spec.files {
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let committed = staging.join(&f.name);
        let part = part_path(&staging, f);
        let committed_len = file_len(&committed);
        if committed_len.is_some_and(|n| n != f.size) {
            let _ = fs::remove_file(&committed);
        }
        match plan_file(file_len(&part), committed_len, f.size) {
            FilePlan::Skip => continue,
            FilePlan::Restart => {
                // 比應有的大:bytes_present 本來就沒把它算進進度
                let _ = fs::remove_file(&part);
            }
            FilePlan::Fresh if file_len(&part).is_some_and(|n| n > 0) => {
                // 小檔的部分暫存檔不續傳:扣回進度後從頭下載
                rep.done -= file_len(&part).unwrap_or(0).min(f.size);
                let _ = fs::remove_file(&part);
            }
            FilePlan::Fresh | FilePlan::Resume(_) | FilePlan::VerifyOnly => {}
        }
        if file_len(&part) != Some(f.size) {
            let a = match &agent {
                Some(a) => a,
                None => agent.insert(build_agent(opts)?),
            };
            fetch_part(a, &spec.url(opts.base_url.as_deref().unwrap_or(&spec.base_url), f), &part, f, opts, &mut rep)?;
        }
        if ctl.cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        rep.emit(Phase::Verifying, Some(f.name.to_string()));
        let ok = sha256_file(&part).map_err(io_err)? == f.sha256 && file_len(&part) == Some(f.size);
        if !ok {
            let _ = fs::remove_file(&part);
            return Err(DownloadError::Verify { file: f.name.to_string() });
        }
        retry_rename(&part, &committed, RENAME_MAX_WAIT).map_err(io_err)?;
        rep.speed_mark = (Instant::now(), rep.done); // 校驗時間不算進下載速度
    }
    install(spec, &staging, models_dir)?;
    if spec.unzip {
        let dir = models_dir.join(&spec.dir_name);
        for f in spec.files.iter().filter(|f| f.name.ends_with(".zip")) {
            extract_zip(&dir.join(&f.name), &dir).map_err(io_err)?;
        }
    }
    if let Some(parent) = staging.parent() {
        let _ = fs::remove_dir(parent); // `.download` 空了才刪得掉
    }
    rep.done = total;
    ctl.done.store(total, Ordering::Relaxed);
    rep.emit(Phase::Done, None);
    Ok(())
}
