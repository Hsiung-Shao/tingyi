//! llama-server 子程序生命週期:啟動、等待就緒、結束並確認已死。
//!
//! Tauri 與 bench CLI 共用。結束採「kill → 等待 → 再查」,
//! 不信任單次 kill 的回報(Windows 上 crash-handler 狀態的程序會誤報已結束)。

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct LlamaServerSpec {
    pub exe: PathBuf,
    pub model: PathBuf,
    pub port: u16,
    /// `-ngl`;預設 "all"。
    pub gpu_layers: String,
    pub ctx_size: u32,
    pub parallel: u32,
    /// 額外參數(例如 `--jinja`)。
    pub extra_args: Vec<String>,
    pub log_file: Option<PathBuf>,
    /// `--api-key`;共用伺服器給瀏覽器擴充直連時用,避免其他網頁偷用本機 GPU。
    pub api_key: Option<String>,
}

impl LlamaServerSpec {
    pub fn new(model: PathBuf, port: u16) -> Self {
        // 走原始 /completion 的模型(Seed-X、TranslateGemma)不需要 chat 範本;
        // TranslateGemma 的範本在 llama-server 開 --jinja 時甚至會解析失敗而啟動不了。
        let name = model.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let extra_args = if crate::mt::PromptStyle::for_model(name).raw_completion() {
            vec!["--no-jinja".into()]
        } else {
            vec!["--jinja".into()]
        };
        Self {
            exe: crate::paths::llama_server_exe(),
            model,
            port,
            gpu_layers: "all".into(),
            ctx_size: 4096,
            parallel: 1,
            extra_args,
            log_file: None,
            api_key: None,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// llama-server 的完整參數(不含 exe)。
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "-m".into(),
            self.model.to_string_lossy().into_owned(),
            "-ngl".into(),
            self.gpu_layers.clone(),
            "-c".into(),
            self.ctx_size.to_string(),
            "-np".into(),
            self.parallel.to_string(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            self.port.to_string(),
        ];
        a.extend(self.extra_args.iter().cloned());
        if let Some(k) = &self.api_key {
            a.push("--api-key".into());
            a.push(k.clone());
        }
        a
    }

    /// 回修模式:兩個 slot(快路徑 0、回修 1),ctx 加大到 6144 讓每個 slot 仍有 3072 token
    /// 放得下 1600 字前文 + 指令 + 輸出(`-np` 會把 KV 平分)。
    pub fn with_refine(mut self) -> Self {
        self.parallel = 2;
        self.ctx_size = 6144;
        self
    }
}

pub struct LlamaServerProcess {
    child: Child,
    spec: LlamaServerSpec,
    started_at: Instant,
}

impl LlamaServerProcess {
    pub fn spawn(spec: LlamaServerSpec) -> Result<Self> {
        if !spec.exe.exists() {
            return Err(anyhow!("找不到 llama-server:{}", spec.exe.display()));
        }
        if !spec.model.exists() {
            return Err(anyhow!("找不到模型:{}", spec.model.display()));
        }
        let mut cmd = Command::new(&spec.exe);
        cmd.args(spec.args())
            .current_dir(spec.exe.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null());
        match &spec.log_file {
            Some(p) => {
                let f = std::fs::File::create(p)
                    .with_context(|| format!("建立 log {}", p.display()))?;
                let f2 = f.try_clone()?;
                cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().context("spawn llama-server")?;
        tracing::info!(
            "llama-server 啟動 pid={} model={} port={}",
            child.id(),
            spec.model.display(),
            spec.port
        );
        Ok(Self {
            child,
            spec,
            started_at: Instant::now(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 給 Job Object 登錄子程序用。
    pub(crate) fn child(&self) -> &Child {
        &self.child
    }

    pub fn spec(&self) -> &LlamaServerSpec {
        &self.spec
    }

    pub fn base_url(&self) -> String {
        self.spec.base_url()
    }

    /// 阻塞等待 `/health` 回 200;模型載入期間會回 503。
    ///
    /// 用 std TcpStream 手寫最小 HTTP 請求,不依賴 reqwest::blocking,
    /// 因此在 tokio runtime 內(例如 spawn_blocking 或直接呼叫)都安全。
    pub fn wait_ready_blocking(&mut self, max_wait: Duration) -> Result<Duration> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(anyhow!("llama-server 提前結束:{status}"));
            }
            if health_ok(self.spec.port) {
                return Ok(self.started_at.elapsed());
            }
            if self.started_at.elapsed() > max_wait {
                return Err(anyhow!("llama-server 在 {max_wait:?} 內未就緒"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// 結束程序並確認已死;回傳是否確認結束。
    pub fn shutdown(&mut self) -> bool {
        let _ = self.child.kill();
        for _ in 0..25 {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => return false,
            }
        }
        tracing::warn!("llama-server pid={} kill 後 2.5 秒仍存活", self.pid());
        false
    }
}

/// 對 `127.0.0.1:port` 發 `GET /health`,狀態列為 200 才算就緒。
pub fn health_ok(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 64];
    let Ok(n) = s.read(&mut buf) else { return false };
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
}

impl Drop for LlamaServerProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 對 `127.0.0.1:port` 發 GET,只要收到任何 HTTP 回應就算 listen 中(whisper-server 沒有 /health,
/// 模型載入期間尚未開 listen,開了就代表就緒)。
pub fn http_responding(port: u16, path: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 16];
    matches!(s.read(&mut buf), Ok(n) if n >= 8 && buf.starts_with(b"HTTP/"))
}

/// whisper.cpp 的 whisper-server 子程序設定。
#[derive(Debug, Clone)]
pub struct WhisperServerSpec {
    pub exe: PathBuf,
    pub model: PathBuf,
    pub port: u16,
    pub threads: u32,
    /// 預設語言;每次請求可用 multipart `language` 覆蓋。
    pub language: String,
    pub extra_args: Vec<String>,
    pub log_file: Option<PathBuf>,
}

impl WhisperServerSpec {
    pub fn new(model: PathBuf, port: u16) -> Self {
        Self {
            exe: crate::paths::whisper_server_exe(),
            model,
            port,
            threads: 4,
            language: "auto".into(),
            extra_args: vec![],
            log_file: None,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

pub struct WhisperServerProcess {
    child: Child,
    spec: WhisperServerSpec,
    started_at: Instant,
}

impl WhisperServerProcess {
    pub fn spawn(spec: WhisperServerSpec) -> Result<Self> {
        if !spec.exe.exists() {
            return Err(anyhow!("找不到 whisper-server:{}(請執行 scripts/build-whisper.ps1)", spec.exe.display()));
        }
        if !spec.model.exists() {
            return Err(anyhow!("找不到 Whisper 模型:{}", spec.model.display()));
        }
        let mut cmd = Command::new(&spec.exe);
        cmd.arg("-m")
            .arg(&spec.model)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(spec.port.to_string())
            .arg("-t")
            .arg(spec.threads.to_string())
            .arg("-l")
            .arg(&spec.language)
            .args(&spec.extra_args)
            .current_dir(spec.exe.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null());
        match &spec.log_file {
            Some(p) => {
                let f = std::fs::File::create(p)
                    .with_context(|| format!("建立 log {}", p.display()))?;
                let f2 = f.try_clone()?;
                cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().context("spawn whisper-server")?;
        tracing::info!(
            "whisper-server 啟動 pid={} model={} port={}",
            child.id(),
            spec.model.display(),
            spec.port
        );
        Ok(Self {
            child,
            spec,
            started_at: Instant::now(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn base_url(&self) -> String {
        self.spec.base_url()
    }

    pub fn wait_ready_blocking(&mut self, max_wait: Duration) -> Result<Duration> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(anyhow!("whisper-server 提前結束:{status}"));
            }
            if http_responding(self.spec.port, "/") {
                return Ok(self.started_at.elapsed());
            }
            if self.started_at.elapsed() > max_wait {
                return Err(anyhow!("whisper-server 在 {max_wait:?} 內未就緒"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn shutdown(&mut self) -> bool {
        let _ = self.child.kill();
        for _ in 0..25 {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => return false,
            }
        }
        tracing::warn!("whisper-server pid={} kill 後 2.5 秒仍存活", self.pid());
        false
    }
}

impl Drop for WhisperServerProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}
