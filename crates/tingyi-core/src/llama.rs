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
}

impl LlamaServerSpec {
    pub fn new(model: PathBuf, port: u16) -> Self {
        Self {
            exe: crate::paths::llama_server_exe(),
            model,
            port,
            gpu_layers: "all".into(),
            ctx_size: 4096,
            parallel: 1,
            extra_args: vec!["--jinja".into()],
            log_file: None,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
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
        cmd.arg("-m")
            .arg(&spec.model)
            .arg("-ngl")
            .arg(&spec.gpu_layers)
            .arg("-c")
            .arg(spec.ctx_size.to_string())
            .arg("-np")
            .arg(spec.parallel.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(spec.port.to_string())
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
