//! 翻譯模型下載規格(S 系列)與 OBS 事件緩衝/路由(O 系列)。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use sha2::{Digest, Sha256};
use tingyi_core::model_download::{
    download_model, mt_spec_for, DownloadCtl, DownloadOptions, FileSpec, ModelSpec, DEFAULT_BASE_URL, TRANSLATE_PACKAGES,
};
use tingyi_core::obs::{route, EventLog, Route};
use tingyi_core::paths::{find_gguf, KNOWN_MT_MODELS};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 暫存目錄:pid + 序號命名,離開作用域時刪除。
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("tingyi_mt_obs_{}_{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// 極簡 HTTP 伺服器:依路徑回 200 + Content-Length + body(忽略 Range),回傳埠號。
fn serve(routes: Vec<(String, Vec<u8>)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let routes = routes.clone();
            thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                match routes.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
                let _ = stream.flush();
            });
        }
    });
    port
}

// S1:KNOWN_MT_MODELS 每個翻譯模型都有下載規格(大小、sha256、檔名正確)且列在 TRANSLATE_PACKAGES
#[test]
fn s1_every_known_mt_model_has_download_spec() {
    assert_eq!(KNOWN_MT_MODELS.len(), 10, "已知翻譯模型應為 10 個");
    for (name, _) in KNOWN_MT_MODELS {
        let spec = mt_spec_for(name).unwrap_or_else(|| panic!("{name} 應有下載規格"));
        assert_eq!(spec.files.len(), 1, "{name} 應只有一個檔");
        let f = &spec.files[0];
        assert!(f.size > 0, "{name} 大小應大於 0");
        assert_eq!(f.sha256.len(), 64, "{name} sha256 應為 64 字元");
        assert!(
            f.sha256.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{name} sha256 應為小寫 hex:{}",
            f.sha256
        );
        assert_eq!(f.name, format!("{name}.gguf"), "{name} 本機檔名應為 <name>.gguf");
        assert!(
            TRANSLATE_PACKAGES.iter().any(|p| p.dir_name == spec.dir_name && p.files.len() == 1 && p.files[0].name == f.name),
            "{name} 應列在 TRANSLATE_PACKAGES"
        );
    }
}

// S2:下載網址正確(Sakura 遠端改名、TranslateGemma 改名、Hy-MT2 同名不改)
#[test]
fn s2_download_urls_use_remote_names() {
    let sakura = mt_spec_for("Sakura-7B-Qwen2.5-v1.0-IQ4_XS").expect("Sakura 7B 應有規格");
    assert_eq!(
        sakura.url(DEFAULT_BASE_URL, &sakura.files[0]),
        "https://huggingface.co/SakuraLLM/Sakura-7B-Qwen2.5-v1.0-GGUF/resolve/24393225dfab4750c6fb9ec2468fb76ad3d2dd1d/sakura-7b-qwen2.5-v1.0-iq4xs.gguf"
    );

    let gemma = mt_spec_for("TranslateGemma-4B-it-Q4_K_M").expect("TranslateGemma 4B 應有規格");
    let gemma_url = gemma.url(DEFAULT_BASE_URL, &gemma.files[0]);
    assert!(gemma_url.ends_with("/translategemma-4b-it.Q4_K_M.gguf"), "實際網址:{gemma_url}");

    let hy = mt_spec_for("Hy-MT2-7B-Q4_K_M").expect("Hy-MT2 7B 應有規格");
    let hy_url = hy.url(DEFAULT_BASE_URL, &hy.files[0]);
    assert!(hy_url.ends_with("/Hy-MT2-7B-Q4_K_M.gguf"), "實際網址:{hy_url}");
}

// S3:遠端改名的檔下載後以本機名存放,find_gguf 找得到且內容一致
#[test]
fn s3_renamed_remote_file_installs_under_local_name() {
    let body = b"fake-gguf-bytes-for-sakura-0123456789".to_vec();
    let base_spec: &'static ModelSpec = mt_spec_for("Sakura-7B-Qwen2.5-v1.0-IQ4_XS").expect("Sakura 7B 應有規格");

    let route_path = format!("/{}/resolve/{}/sakura-7b-qwen2.5-v1.0-iq4xs.gguf", base_spec.repo, base_spec.commit);
    let port = serve(vec![(route_path, body.clone())]);

    let files: &'static [FileSpec] = Box::leak(
        vec![FileSpec {
            name: "Sakura-7B-Qwen2.5-v1.0-IQ4_XS.gguf",
            size: body.len() as u64,
            sha256: Box::leak(sha_hex(&body).into_boxed_str()),
        }]
        .into_boxed_slice(),
    );
    let spec: &'static ModelSpec = Box::leak(Box::new(ModelSpec { files, ..*base_spec }));

    let tmp = Tmp::new();
    let models = tmp.path().join("models");
    fs::create_dir_all(&models).unwrap();
    let opts = DownloadOptions {
        base_url: format!("http://127.0.0.1:{port}"),
        retries: 0,
        retry_backoff: std::time::Duration::from_millis(10),
        read_timeout: std::time::Duration::from_secs(10),
        connect_timeout: std::time::Duration::from_secs(5),
        ..DownloadOptions::default()
    };
    let ctl = DownloadCtl::default();
    let r = download_model(spec, &models, &opts, &ctl, &mut |_| {});
    assert!(r.is_ok(), "下載應成功:{r:?}");

    let found = find_gguf(&models, "Sakura-7B-Qwen2.5-v1.0-IQ4_XS");
    assert!(found.exists(), "find_gguf 應找到本機名檔案:{found:?}");
    assert_eq!(fs::read(&found).unwrap(), body, "內容應與伺服器一致");
}

// O1:push 3 筆後 snapshot 依序回傳 3 筆
#[test]
fn o1_snapshot_keeps_push_order() {
    let mut log = EventLog::new(10);
    log.push("a".to_string());
    log.push("b".to_string());
    log.push("c".to_string());
    assert_eq!(log.snapshot(), vec!["a".to_string(), "b".to_string(), "c".to_string()]);
}

// O1b:超過 cap 時丟最舊的,只剩後 2 筆
#[test]
fn o1b_over_cap_drops_oldest() {
    let mut log = EventLog::new(2);
    log.push("a".to_string());
    log.push("b".to_string());
    log.push("c".to_string());
    assert_eq!(log.snapshot(), vec!["b".to_string(), "c".to_string()]);
}

// O2:reset 之後 snapshot 只含 reset 後才 push 的事件
#[test]
fn o2_reset_clears_earlier_events() {
    let mut log = EventLog::new(10);
    log.push("a".to_string());
    log.push("b".to_string());
    log.reset();
    log.push("c".to_string());
    assert_eq!(log.snapshot(), vec!["c".to_string()]);
}

// O3:Host 必須是本機位址,偽造 Host 一律拒絕
#[test]
fn o3_host_must_be_loopback() {
    assert_eq!(route("GET", "/events", Some("evil.com"), false), Route::Forbidden);
    assert_eq!(route("GET", "/events", Some("127.0.0.1.evil.com"), false), Route::Forbidden);
    assert_eq!(route("GET", "/events", None, false), Route::Forbidden);
    assert_eq!(route("GET", "/events", Some("127.0.0.1:17880"), false), Route::Events);
    assert_eq!(route("GET", "/events", Some("localhost"), false), Route::Events);
}

// O4:頁面/事件/靜態資源路由正確,路徑穿越與非 GET 一律 NotFound
#[test]
fn o4_routes_and_path_traversal() {
    let host = Some("127.0.0.1:17880");
    assert_eq!(route("GET", "/history?n=6", host, false), Route::Page);
    assert_eq!(route("GET", "/subtitle", host, false), Route::Page);
    assert_eq!(route("POST", "/events", host, false), Route::NotFound);

    assert_eq!(route("GET", "/assets/obs-abc.js", host, false), Route::Asset("assets/obs-abc.js".to_string()));
    assert_eq!(route("GET", "/assets/../Cargo.toml", host, false), Route::NotFound);
    assert_eq!(route("GET", "/Cargo.toml", host, false), Route::NotFound);

    assert_eq!(route("GET", "/src/Obs.tsx", host, true), Route::Asset("src/Obs.tsx".to_string()));
    assert_eq!(route("GET", "/../x", host, true), Route::NotFound);
}
