//! 整合測試共用:只 mock 網路的假 HTTP 伺服器(std TcpListener)。
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Req {
    pub path: String,
    /// header 名稱一律小寫
    pub headers: Vec<(String, String)>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Req> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 || buf.len() > 64 * 1024 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.split("\r\n");
    let path = lines.next()?.split_whitespace().nth(1)?.to_string();
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    Some(Req { path, headers })
}

/// `content_len` 可大於 `body.len()`:模擬送到一半就斷線。
pub fn send(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], content_len: usize, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {content_len}\r\nConnection: close\r\n\r\n"));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// 遵守 Range 的一般檔案回應:有 `Range: bytes=n-` 回 206,否則 200。
pub fn serve_body(req: &Req, stream: &mut TcpStream, body: &[u8]) {
    let total = body.len();
    let start = req
        .header("range")
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse::<usize>().ok());
    if let Some(start) = start.filter(|s| *s < total) {
        let cr = format!("bytes {}-{}/{}", start, total - 1, total);
        send(stream, 206, &[("Content-Range", cr)], total - start, &body[start..]);
        return;
    }
    send(stream, 200, &[], total, body);
}

/// 依序處理連線的假伺服器;handler 收到 (第幾個請求(從 0 起), 請求, 連線)。
pub struct Server {
    pub base: String,
    reqs: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start<F>(handler: F) -> Server
    where
        F: Fn(usize, &Req, &mut TcpStream) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let reqs: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (r2, s2) = (reqs.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = conn else { continue };
                let Some(req) = read_request(&mut stream) else { continue };
                let idx = {
                    let mut g = r2.lock().unwrap();
                    g.push(req.clone());
                    g.len() - 1
                };
                handler(idx, &req, &mut stream);
            }
        });
        Server { base: format!("http://{addr}"), reqs, stop, addr, thread: Some(thread) }
    }

    pub fn requests(&self) -> Vec<Req> {
        self.reqs.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr); // 喚醒 accept
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
