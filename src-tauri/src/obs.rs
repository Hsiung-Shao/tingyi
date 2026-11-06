//! OBS 瀏覽器來源:本機 HTTP 伺服器,提供 `/history`(辨識紀錄)、`/subtitle`(字幕)兩頁與 `/events`(SSE)。
//! 只綁 127.0.0.1、只收本機 Host;預設關閉,設定頁打開才 listen。
//! 路由與事件緩衝的規則在 `tingyi_core::obs`(有測試);這裡只做連線。

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tingyi_core::obs::{route, EventLog, Route};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

use crate::state::AppState;

/// 預設埠;被佔用就往後找。
const BASE_PORT: u16 = 17880;
const PORT_TRIES: u16 = 20;
/// 重播緩衝上限(一句約 3–6 則事件,足夠補回數百句)。
const LOG_CAP: usize = 2000;
const RESET: &str = r#"{"type":"reset"}"#;
/// 開發版頁面資源由 vite 提供。
const VITE: &str = "localhost:1420";

pub struct Obs {
    log: Mutex<EventLog>,
    tx: broadcast::Sender<String>,
    server: Mutex<Option<(u16, tokio::task::JoinHandle<()>)>>,
    /// 每次關閉加 1:已連上的 SSE 連線看到就結束(關掉開關後 OBS 不再收到字幕)。
    closed: watch::Sender<u64>,
    /// 開關的開啟流程一次只跑一個(開發版 StrictMode 會同時送兩次)。
    toggling: tokio::sync::Mutex<()>,
}

impl Default for Obs {
    fn default() -> Self {
        Self {
            log: Mutex::new(EventLog::new(LOG_CAP)),
            tx: broadcast::channel(512).0,
            server: Mutex::new(None),
            closed: watch::channel(0).0,
            toggling: tokio::sync::Mutex::new(()),
        }
    }
}

impl Obs {
    /// 管線事件(已序列化的 JSON);沒開伺服器也照記,打開時才補得回本回合。
    pub fn push(&self, line: String) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.push(line.clone());
        let _ = self.tx.send(line);
    }

    /// 新回合(清除上一輪紀錄)時呼叫。
    pub fn reset(&self) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.reset();
        let _ = self.tx.send(RESET.to_string());
    }

    /// 在同一把鎖內訂閱並取快照:快照之後的事件一定會從訂閱收到,不漏也不重複。
    fn subscribe(&self) -> (Vec<String>, broadcast::Receiver<String>) {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        (log.snapshot(), self.tx.subscribe())
    }
}

#[derive(Serialize)]
pub struct ObsStatus {
    pub enabled: bool,
    pub port: Option<u16>,
}

fn status(obs: &Obs) -> ObsStatus {
    let port = obs.server.lock().unwrap().as_ref().map(|(p, _)| *p);
    ObsStatus { enabled: port.is_some(), port }
}

#[tauri::command]
pub fn obs_status(state: State<'_, AppState>) -> ObsStatus {
    status(&state.obs)
}

#[tauri::command]
pub async fn set_obs_enabled(app: AppHandle, enabled: bool) -> Result<ObsStatus, String> {
    let state = app.state::<AppState>();
    let _one = state.obs.toggling.lock().await;
    if !enabled {
        if let Some((_, task)) = state.obs.server.lock().unwrap().take() {
            task.abort();
            state.obs.closed.send_modify(|n| *n += 1);
        }
        return Ok(status(&state.obs));
    }
    if state.obs.server.lock().unwrap().is_some() {
        return Ok(status(&state.obs));
    }
    let mut listener = None;
    for port in BASE_PORT..BASE_PORT + PORT_TRIES {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)).await {
            listener = Some((port, l));
            break;
        }
    }
    let Some((port, listener)) = listener else {
        return Err(format!("{BASE_PORT}–{} 的埠都被佔用,無法開啟直播網址", BASE_PORT + PORT_TRIES - 1));
    };
    tracing::info!("OBS 網址:http://127.0.0.1:{port}/history、/subtitle");
    let app2 = app.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // 資源暫時用盡等持續性錯誤:稍等再收,不空轉
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            };
            let app = app2.clone();
            tokio::spawn(async move {
                if let Err(e) = serve(&app, stream).await {
                    tracing::debug!("OBS 連線結束:{e}");
                }
            });
        }
    });
    *state.obs.server.lock().unwrap() = Some((port, task));
    Ok(status(&state.obs))
}

/// 讀請求標頭(最多 8 KB),回傳 (method, target, host)。
async fn read_head(stream: &mut TcpStream) -> std::io::Result<(String, String, Option<String>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > 8192 {
            return Err(std::io::Error::other("標頭太大"));
        }
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk)).await??;
        if n == 0 {
            return Err(std::io::Error::other("連線提早關閉"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_string();
    let target = first.next().unwrap_or("").to_string();
    let host = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim().to_string());
    Ok((method, target, host))
}

async fn respond(stream: &mut TcpStream, code: &str, mime: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await
}

async fn serve(app: &AppHandle, mut stream: TcpStream) -> std::io::Result<()> {
    let (method, target, host) = read_head(&mut stream).await?;
    let dev = cfg!(debug_assertions);
    match route(&method, &target, host.as_deref(), dev) {
        Route::Forbidden => respond(&mut stream, "403 Forbidden", "text/plain", b"forbidden").await,
        Route::NotFound => respond(&mut stream, "404 Not Found", "text/plain", b"not found").await,
        Route::Events => events(app, stream).await,
        Route::Page => file(app, stream, "obs.html", dev).await,
        Route::Asset(path) => file(app, stream, &path, dev).await,
    }
}

async fn file(app: &AppHandle, mut stream: TcpStream, path: &str, dev: bool) -> std::io::Result<()> {
    if dev {
        return proxy_vite(stream, path).await;
    }
    let resolver = app.asset_resolver();
    // 找不到時 Tauri 會退回主視窗的 index.html;那不是這裡要的檔,當 404
    let index = resolver.get("index.html".to_string()).map(|a| a.bytes);
    match resolver.get(path.to_string()) {
        Some(a) if Some(&a.bytes) != index.as_ref() => respond(&mut stream, "200 OK", &a.mime_type, &a.bytes).await,
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"not found").await,
    }
}

/// 開發版:把請求原樣轉給 vite,回應原封不動傳回。
async fn proxy_vite(mut stream: TcpStream, path: &str) -> std::io::Result<()> {
    let mut up = TcpStream::connect(VITE).await?;
    up.write_all(format!("GET /{path} HTTP/1.1\r\nHost: {VITE}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
    tokio::io::copy(&mut up, &mut stream).await?;
    Ok(())
}

/// SSE:先送 reset 與本回合快照,之後即時轉發;每 15 秒送註解保持連線。
async fn events(app: &AppHandle, mut stream: TcpStream) -> std::io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n")
        .await?;
    let mut closed = app.state::<AppState>().obs.closed.subscribe();
    let (snapshot, mut rx) = app.state::<AppState>().obs.subscribe();
    let mut out = format!("data: {RESET}\n\n");
    for line in snapshot {
        out.push_str(&format!("data: {line}\n\n"));
    }
    stream.write_all(out.as_bytes()).await?;
    let mut beat = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(line) => stream.write_all(format!("data: {line}\n\n").as_bytes()).await?,
                // 跟不上(OBS 卡住):重送快照,畫面以最新狀態為準
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (snapshot, fresh) = app.state::<AppState>().obs.subscribe();
                    rx = fresh;
                    let mut out = format!("data: {RESET}\n\n");
                    for line in snapshot {
                        out.push_str(&format!("data: {line}\n\n"));
                    }
                    stream.write_all(out.as_bytes()).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            _ = beat.tick() => stream.write_all(b": ping\n\n").await?,
            // 使用者關掉直播開關
            _ = closed.changed() => return Ok(()),
        }
    }
}
