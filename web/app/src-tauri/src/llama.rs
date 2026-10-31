//! llama-server 子程序:啟動、等待就緒、結束並確認已死(改自 tingyi `llama.rs`)。
//!
//! 只聽 127.0.0.1,並帶每次啟動隨機產生的 `--api-key`,其他網頁不能偷用本機 GPU。
//! 子程序放進 Job Object,本程式被強制結束時一起結束,不留孤兒。

use crate::job::Job;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 擴充送翻時用的模型名稱(llama-server 不檢查,只是讓請求格式完整)
pub const MODEL_NAME: &str = "hy-mt2";

#[derive(Debug, Clone)]
pub struct LlamaSpec {
    pub exe: PathBuf,
    pub model: PathBuf,
    pub port: u16,
    pub api_key: String,
    pub ctx_size: u32,
    pub parallel: u32,
    pub log_file: Option<PathBuf>,
}

impl LlamaSpec {
    /// 預設 ctx 8192、2 個 slot,api key 每次新產生
    pub fn new(exe: PathBuf, model: PathBuf, port: u16) -> Self {
        Self { exe, model, port, api_key: new_api_key(), ctx_size: 8192, parallel: 2, log_file: None }
    }

    /// llama-server 的完整參數(不含 exe):
    /// `-m <model> -ngl all -c <ctx> -np <parallel> --host 127.0.0.1 --port <port> --jinja --api-key <key>`
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["-m".into(), self.model.display().to_string()];
        for (k, v) in [
            ("-ngl", "all".to_string()),
            ("-c", self.ctx_size.to_string()),
            ("-np", self.parallel.to_string()),
            ("--host", "127.0.0.1".to_string()),
            ("--port", self.port.to_string()),
        ] {
            a.push(k.into());
            a.push(v);
        }
        a.extend(["--jinja".into(), "--api-key".into(), self.api_key.clone()]);
        a
    }

    /// 擴充直連用的 OpenAI 相容端點
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

/// 32 bytes 隨機值的 hex 字串(64 字元)
pub fn new_api_key() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 向系統要一個目前空閒的本機 port
pub fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

pub struct LlamaProcess {
    child: Child,
    spec: LlamaSpec,
    started_at: Instant,
}

impl LlamaProcess {
    pub fn spawn(spec: LlamaSpec, job: &Job) -> Result<Self> {
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
                let f = std::fs::File::create(p).with_context(|| format!("建立 log {}", p.display()))?;
                let f2 = f.try_clone()?;
                cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        crate::job::hide_console(&mut cmd);
        let child = cmd.spawn().context("spawn llama-server")?;
        if let Err(e) = job.assign(&child) {
            tracing::warn!("llama-server 放入 Job Object 失敗:{e}");
        }
        tracing::info!("llama-server 啟動 pid={} port={}", child.id(), spec.port);
        Ok(Self { child, spec, started_at: Instant::now() })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn spec(&self) -> &LlamaSpec {
        &self.spec
    }

    /// 阻塞等待 `/health` 回 200;模型載入期間會回 503
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

    /// 已經結束了沒(崩潰偵測用)
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// 結束程序並確認已死;回傳是否確認結束
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

impl Drop for LlamaProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 對 `127.0.0.1:port` 發 `GET /health`,狀態列為 200 才算就緒(/health 不需 api key)
pub fn health_ok(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let Ok(n) = s.read(&mut buf) else { return false };
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200")
}
