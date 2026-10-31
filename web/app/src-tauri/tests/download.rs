//! download_package 的整合測試(只 mock 網路:假 HTTP 伺服器)。
#![allow(non_snake_case)]

mod common;

use atx_app_lib::download::{download_package, package_ready, DownloadCtl, DownloadError, DownloadOptions, FileSpec, Package};
use common::{send, serve_body, Server};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::net::Shutdown;
use std::time::Duration;

fn hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn opts(base: &str) -> DownloadOptions {
    DownloadOptions {
        base_url: Some(base.to_string()),
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_millis(500),
        retries: 3,
        retry_backoff: Duration::from_millis(10),
    }
}

fn pkg(file: FileSpec, unzip: bool, marker: &str) -> Package {
    Package {
        id: "t".into(),
        label: "測試套件".into(),
        dir_name: "pkg".into(),
        base_url: "http://127.0.0.1:1".into(), // 不該被用到:測試一律以 opts.base_url 覆寫
        files: vec![file],
        unzip,
        ready_marker: marker.into(),
    }
}

fn run(p: &Package, parent: &std::path::Path, o: &DownloadOptions) -> Result<(), DownloadError> {
    let ctl = DownloadCtl::default();
    download_package(p, parent, o, &ctl, &mut |_| {})
}

/// 2 MiB 偽隨機資料(大於 1 MiB 才會續傳)
fn big_data() -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..(2usize << 20))
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

// D1:伺服器內容的 SHA 與 FileSpec 不符 → 回 Verify 錯誤、不提交到正式目錄
#[test]
fn D1_校驗不符時回Verify且不提交檔案() {
    let served = b"hello world".to_vec();
    let server = Server::start(move |_, req, s| serve_body(req, s, &served));
    let tmp = tempfile::tempdir().unwrap();
    let file = FileSpec {
        name: "m.bin".into(),
        path: "r/m.bin".into(),
        size: 11,                        // 長度正確
        sha256: hex(b"another content"), // 但 SHA 不是伺服器送的內容
    };
    let p = pkg(file, false, "m.bin");

    let r = run(&p, tmp.path(), &opts(&server.base));

    assert!(matches!(r, Err(DownloadError::Verify { .. })), "應回 Verify,實際:{r:?}");
    assert!(!tmp.path().join("pkg").join("m.bin").exists(), "壞檔不該被提交到正式目錄");
    assert!(!package_ready(&p, tmp.path()), "壞檔不該被視為已安裝");
}

// D2:第一次送一半就斷線,之後以 Range 續傳,最終內容與 SHA 正確
#[test]
fn D2_中斷後以Range續傳且內容正確() {
    let data = big_data();
    let served = data.clone();
    let server = Server::start(move |idx, req, s| {
        if idx == 0 {
            let half = served.len() / 2;
            send(s, 200, &[], served.len(), &served[..half]);
            let _ = s.shutdown(Shutdown::Both); // 只送一半就斷線
        } else {
            serve_body(req, s, &served);
        }
    });
    let tmp = tempfile::tempdir().unwrap();
    let file = FileSpec { name: "big.bin".into(), path: "r/big.bin".into(), size: data.len() as u64, sha256: hex(&data) };
    let p = pkg(file, false, "big.bin");

    let r = run(&p, tmp.path(), &opts(&server.base));

    assert!(r.is_ok(), "斷線後應自動續傳成功,實際:{r:?}");
    let reqs = server.requests();
    assert!(reqs.len() >= 2, "至少要有第二次請求,實際 {} 次", reqs.len());
    assert!(reqs[0].header("range").is_none(), "首次請求不該帶 Range");
    let range = reqs[1].header("range").expect("續傳請求必須帶 Range");
    let n: usize = range
        .strip_prefix("bytes=")
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse().ok())
        .expect("Range 格式應為 bytes=<n>-");
    assert!(n > 0 && n < data.len(), "續傳起點應是已收到的位元組數,實際 {n}");
    let got = std::fs::read(tmp.path().join("pkg").join("big.bin")).unwrap();
    assert!(got == data, "續傳後檔案內容必須與原始資料完全相同");
    assert!(package_ready(&p, tmp.path()), "下載完成後應視為已安裝");
}

// D3:llama zip 下載後解壓出 llama-server.exe(及同層其他檔案)
#[test]
fn D3_zip解壓出llama_server執行檔() {
    let exe_bytes = b"MZ-fake-llama-server-binary".to_vec();
    let dll_bytes = b"fake-ggml-dll".to_vec();
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let zo = zip::write::SimpleFileOptions::default();
    w.start_file("llama-server.exe", zo).unwrap();
    w.write_all(&exe_bytes).unwrap();
    w.start_file("ggml.dll", zo).unwrap();
    w.write_all(&dll_bytes).unwrap();
    let zip_bytes = w.finish().unwrap().into_inner();

    let served = zip_bytes.clone();
    let server = Server::start(move |_, req, s| serve_body(req, s, &served));
    let tmp = tempfile::tempdir().unwrap();
    let file = FileSpec { name: "llama.zip".into(), path: "r/llama.zip".into(), size: zip_bytes.len() as u64, sha256: hex(&zip_bytes) };
    let p = pkg(file, true, "llama-server.exe");

    let r = run(&p, tmp.path(), &opts(&server.base));

    assert!(r.is_ok(), "下載解壓應成功,實際:{r:?}");
    let dir = tmp.path().join("pkg");
    assert!(dir.join("llama-server.exe").is_file(), "llama-server.exe 應被解壓出來");
    assert_eq!(std::fs::read(dir.join("llama-server.exe")).unwrap(), exe_bytes);
    assert!(dir.join("ggml.dll").is_file(), "同層的 dll 也要解壓出來");
    assert_eq!(std::fs::read(dir.join("ggml.dll")).unwrap(), dll_bytes);
    assert!(package_ready(&p, tmp.path()), "解壓完成後應視為已安裝");
}
