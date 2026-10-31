//! Chrome Native Messaging host:擴充呼叫 `chrome.runtime.sendNativeMessage` 時,
//! Chrome 以 `tingyi.exe chrome-extension://<id>/` 啟動本程式,經 stdin/stdout 交換訊息。
//!
//! 訊息格式:4 bytes little-endian 長度 + UTF-8 JSON。host 回給 Chrome 的單則上限 1 MB。

use crate::runtime::{self, LlamaStatus, Runtime};
use serde_json::{json, Value};
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const HOST_NAME: &str = "com.jerry.tingyi.host";
/// 擴充喚起程式時帶的參數:背景啟動,並讓網頁翻譯取得共用翻譯伺服器。
pub const WAKE_ARGS: [&str; 2] = ["--background", "--serve-web"];
/// host → Chrome 的單則訊息上限
pub const MAX_OUTGOING: usize = 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    TooLarge(usize),
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooLarge(n) => write!(f, "訊息 {n} bytes 超過 1 MB 上限"),
            FrameError::Io(e) => write!(f, "{e}"),
            FrameError::Json(e) => write!(f, "{e}"),
        }
    }
}

/// 編成一則 native messaging 訊息(長度前綴 + JSON)
pub fn encode(msg: &Value) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg).map_err(FrameError::Json)?;
    if body.len() > MAX_OUTGOING {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// 讀一則訊息;stdin 已關閉(Chrome 斷線)回 Ok(None)
pub fn decode(reader: &mut impl Read) -> Result<Option<Value>, FrameError> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let n = u32::from_le_bytes(len) as usize;
    // 擴充送來的只有短小的 hello;過大的長度多半是管道錯亂,不配置那麼大的記憶體
    if n > MAX_OUTGOING {
        return Err(FrameError::TooLarge(n));
    }
    let mut body = vec![0u8; n];
    reader.read_exact(&mut body).map_err(FrameError::Io)?;
    serde_json::from_slice(&body).map(Some).map_err(FrameError::Json)
}

/// 啟動本機程式(測試換成假的)
pub trait AppLauncher {
    fn launch(&self, args: &[&str]) -> io::Result<()>;
}

/// 判斷 pid 是否還活著(測試換成假的)
pub trait ProcessProbe {
    fn alive(&self, pid: u32) -> bool;
}

pub struct HostCtx<'a> {
    pub runtime_file: PathBuf,
    pub launcher: &'a dyn AppLauncher,
    pub probe: &'a dyn ProcessProbe,
    /// 喚起程式後最多等多久讓 runtime.json 出現
    pub launch_wait: Duration,
    pub poll: Duration,
    pub app_version: String,
}

/// 處理擴充送來的一則請求,回覆給擴充的 JSON。
///
/// `{type:"hello"}` 的回覆:
/// - 成功:`{ok:true, status:"ready"|"downloading"|"starting"|"stopped"|"not-downloaded"|"error",
///   llama:{baseUrl,model,apiKey}|null, slots:[2,3](只在 ready), bridge:{url,token}|null, message, appVersion}`
///   (`llama` 只在 status = ready 時有值;baseUrl 形如 `http://127.0.0.1:<port>/v1`)
/// - 失敗:`{ok:false, error:"app-error"|"bad-request", message}`
///
/// 程式沒在跑(runtime.json 不存在或 pid 已死),或在跑但網頁翻譯還沒取得伺服器(stopped / error)時,
/// 以 `--background --serve-web` 喚起(單一實例會轉交給執行中的程式),等到不再是 starting / stopped 再回覆;
/// 超過 `launch_wait` 回當下狀態,一直沒有 runtime.json 回 app-error。下載中或未下載直接回覆,不喚起。
pub fn handle(req: &Value, ctx: &HostCtx) -> Value {
    if req.get("type").and_then(Value::as_str) != Some("hello") {
        return json!({ "ok": false, "error": "bad-request", "message": "不認得的請求" });
    }
    let running = |ctx: &HostCtx| runtime::read(&ctx.runtime_file).filter(|rt| ctx.probe.alive(rt.pid));
    if let Some(rt) = running(ctx) {
        if !matches!(rt.status, LlamaStatus::Stopped | LlamaStatus::Error) {
            return reply_from_runtime(&rt, &ctx.app_version);
        }
    }
    if let Err(e) = ctx.launcher.launch(&WAKE_ARGS) {
        return json!({ "ok": false, "error": "app-error", "message": format!("無法啟動本機程式:{e}") });
    }
    // 等程式寫出 runtime.json 並開始提供網頁翻譯;模型還在載入就再等一下,逾時就回目前狀態
    let t0 = Instant::now();
    let mut last: Option<Runtime> = None;
    while t0.elapsed() < ctx.launch_wait {
        if let Some(rt) = running(ctx) {
            if !matches!(rt.status, LlamaStatus::Starting | LlamaStatus::Stopped) {
                return reply_from_runtime(&rt, &ctx.app_version);
            }
            last = Some(rt);
        }
        std::thread::sleep(ctx.poll);
    }
    match last {
        Some(rt) => reply_from_runtime(&rt, &ctx.app_version),
        None => json!({
            "ok": false,
            "error": "app-error",
            "message": format!("本機程式在 {} 秒內沒有回應", ctx.launch_wait.as_secs_f32()),
        }),
    }
}

/// runtime 轉成 hello 回覆
pub fn reply_from_runtime(rt: &Runtime, app_version: &str) -> Value {
    let llama = match (&rt.llama, rt.status) {
        (Some(l), LlamaStatus::Ready) => json!({
            "baseUrl": format!("http://127.0.0.1:{}/v1", l.port),
            "model": l.model,
            "apiKey": l.api_key,
        }),
        _ => Value::Null,
    };
    let slots = if llama.is_null() { Value::Null } else { json!(tingyi_core::translate_server::WEB_SLOTS) };
    let bridge = rt.bridge.as_ref().map(|b| json!({ "url": b.url, "token": b.token })).unwrap_or(Value::Null);
    json!({
        "ok": true,
        "status": rt.status,
        "llama": llama,
        "slots": slots,
        "bridge": bridge,
        "message": rt.message,
        "appVersion": app_version,
    })
}
