//! 下拉引擎(Whisper turbo、Qwen3、Cohere)的下載規格、就緒判斷與設定檢查。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use sha2::{Digest, Sha256};
use tingyi_core::asr::{Lang, Qwen3Config, SherpaQwen3};
use tingyi_core::model_download::{
    download_model, engine_packages, package_usable, DownloadCtl, DownloadOptions, FileSpec, ModelSpec, COHERE, QWEN3_06B,
    QWEN3_17B, UNZIP_DONE_MARKER, WHISPER_SERVER, WHISPER_TURBO,
};
use tingyi_core::paths::whisper_server_exe_in;
use tingyi_core::pipeline::{cohere_lang, refine_supported, AsrEngine};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 暫存目錄:pid + 序號命名,離開作用域時刪除。
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("tingyi_engine_dl_{}_{}", std::process::id(), n));
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

fn touch(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"").unwrap();
}

/// 建立指定大小的稀疏檔(只設長度,不真的寫入內容)。
fn sparse(p: &Path, size: u64) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::File::create(p).unwrap().set_len(size).unwrap();
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

// E1:規格含子資料夾檔名時,能從本機伺服器下載並裝到 models/<dir_name>/ 對應位置,內容一致
#[test]
fn e1_nested_file_downloads_and_installs() {
    let vocab = br#"{"hello":1,"world":2}"#.to_vec();
    let onnx = b"fake-onnx-model-bytes-0123456789".to_vec();
    let port = serve(vec![
        ("/tokenizer/vocab.json".to_string(), vocab.clone()),
        ("/a.onnx".to_string(), onnx.clone()),
    ]);

    // sha256 用 sha2 在測試內自算
    let files: &'static [FileSpec] = Box::leak(
        vec![
            FileSpec {
                name: "tokenizer/vocab.json",
                size: vocab.len() as u64,
                sha256: Box::leak(sha_hex(&vocab).into_boxed_str()),
            },
            FileSpec { name: "a.onnx", size: onnx.len() as u64, sha256: Box::leak(sha_hex(&onnx).into_boxed_str()) },
        ]
        .into_boxed_slice(),
    );
    let url_base: &'static str = Box::leak(format!("http://127.0.0.1:{port}").into_boxed_str());
    let spec: &'static ModelSpec = Box::leak(Box::new(ModelSpec {
        id: "e1-test",
        lang: Lang::En,
        label: "E1 測試模型",
        dir_name: "e1-test",
        repo: "x/y",
        commit: "c",
        license: "MIT",
        license_url: "",
        files,
        url_base: Some(url_base),
        unzip: false,
        ready_marker: None,
    }));

    let tmp = Tmp::new();
    let models = tmp.path().join("models");
    fs::create_dir_all(&models).unwrap();
    let opts = DownloadOptions {
        retries: 0,
        retry_backoff: std::time::Duration::from_millis(10),
        read_timeout: std::time::Duration::from_secs(10),
        connect_timeout: std::time::Duration::from_secs(5),
        ..DownloadOptions::default()
    };
    let ctl = DownloadCtl::default();
    let r = download_model(spec, &models, &opts, &ctl, &mut |_| {});
    assert!(r.is_ok(), "下載應成功:{r:?}");

    let got_vocab = fs::read(models.join("e1-test").join("tokenizer").join("vocab.json")).expect("子資料夾檔案應存在");
    assert_eq!(got_vocab, vocab, "子資料夾檔案內容應與伺服器一致");
    let got_onnx = fs::read(models.join("e1-test").join("a.onnx")).expect("根層檔案應存在");
    assert_eq!(got_onnx, onnx);
}

// E2:每個下拉引擎都有下載規格(大小、sha256 格式正確),未知引擎回空
#[test]
fn e2_every_dropdown_engine_has_download_specs() {
    let cases: [(&str, usize); 4] = [("whisper-turbo", 2), ("qwen3-0.6b", 1), ("qwen3-1.7b", 1), ("cohere", 1)];
    for (id, n) in cases {
        let pkgs = engine_packages(id);
        assert!(!pkgs.is_empty(), "{id} 應有下載規格");
        assert_eq!(pkgs.len(), n, "{id} 的套件數");
        for p in &pkgs {
            assert!(!p.files.is_empty(), "{id}/{} 應至少一個檔", p.id);
            for f in p.files {
                assert!(f.size > 0, "{}:{} 的 size 應 > 0", p.id, f.name);
                assert_eq!(f.sha256.len(), 64, "{}:{} 的 sha256 長度", p.id, f.name);
                assert!(
                    f.sha256.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                    "{}:{} 的 sha256 應全為小寫 hex:{}",
                    p.id,
                    f.name,
                    f.sha256
                );
            }
        }
    }
    let ids: Vec<&str> = engine_packages("whisper-turbo").iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![WHISPER_SERVER.id, WHISPER_TURBO.id], "whisper-turbo 順序為 server 再模型");
    assert_eq!(engine_packages("qwen3-0.6b")[0].id, QWEN3_06B.id);
    assert_eq!(engine_packages("qwen3-1.7b")[0].id, QWEN3_17B.id);
    assert_eq!(engine_packages("cohere")[0].id, COHERE.id);
    for id in ["sensevoice", "xyz"] {
        assert!(engine_packages(id).is_empty(), "{id} 不是下拉引擎,應回空");
    }
}

// E3:官方版 whisper-server.exe 放在 Release 子資料夾時,路徑解析與就緒判斷都認得
#[test]
fn e3_official_whisper_in_release_subfolder_is_ready() {
    let tmp = Tmp::new();
    let data = tmp.path();
    let exe = data.join("bin").join("whisper").join("Release").join("whisper-server.exe");
    touch(&exe);
    touch(&data.join("bin").join("whisper").join(UNZIP_DONE_MARKER));
    assert_eq!(whisper_server_exe_in(&data.join("bin")), exe);
    assert!(package_usable(&WHISPER_SERVER, data));
}

// E10:官方 zip 解到一半(exe 已在、沒有解壓完成標記)不算就緒,下載卡才能重下
#[test]
fn e10_official_whisper_without_unzip_marker_is_not_ready() {
    let tmp = Tmp::new();
    let data = tmp.path();
    let official = data.join("bin").join("whisper").join("Release").join("whisper-server.exe");
    touch(&official);
    assert!(!package_usable(&WHISPER_SERVER, data));
    assert_ne!(whisper_server_exe_in(&data.join("bin")), official, "啟動也不該用解到一半的 exe");
}

// E4:自己編的 whisper-server.exe 放在 bin/whisper 照舊就緒;兩處都沒有則不就緒
#[test]
fn e4_self_built_whisper_is_ready_and_missing_is_not() {
    let tmp = Tmp::new();
    let data = tmp.path();
    assert!(!package_usable(&WHISPER_SERVER, data), "兩處都沒有不該就緒");

    let exe = data.join("bin").join("whisper").join("whisper-server.exe");
    touch(&exe);
    assert_eq!(whisper_server_exe_in(&data.join("bin")), exe);
    assert!(package_usable(&WHISPER_SERVER, data));
}

// E5:Whisper 模型檔放 models 根目錄或 models/whisper 都算就緒,都沒放則否
#[test]
fn e5_whisper_model_ready_in_root_or_subfolder() {
    let name = WHISPER_TURBO.files[0].name;
    let size = WHISPER_TURBO.files[0].size;

    let tmp = Tmp::new();
    assert!(!package_usable(&WHISPER_TURBO, tmp.path()), "都沒放不該就緒");

    let root = Tmp::new();
    sparse(&root.path().join("models").join(name), size);
    assert!(package_usable(&WHISPER_TURBO, root.path()), "放 models/ 根目錄應就緒");

    let sub = Tmp::new();
    sparse(&sub.path().join("models").join("whisper").join(name), size);
    assert!(package_usable(&WHISPER_TURBO, sub.path()), "放 models/whisper/ 應就緒");
}

// E6:Qwen3 1.7B 含子資料夾的檔案全在才就緒,少一個就不就緒
#[test]
fn e6_qwen3_17b_nested_files_all_required() {
    let tmp = Tmp::new();
    let data = tmp.path();
    let dir = data.join("models").join(QWEN3_17B.dir_name);
    assert!(!QWEN3_17B.files.is_empty());
    for f in QWEN3_17B.files {
        sparse(&dir.join(f.name), f.size);
    }
    assert!(package_usable(&QWEN3_17B, data), "全部都在應就緒");

    let victim = QWEN3_17B.files.last().unwrap();
    fs::remove_file(dir.join(victim.name)).unwrap();
    assert!(!package_usable(&QWEN3_17B, data), "缺 {} 應不就緒", victim.name);
}

// E7:Qwen3 的 tokenizer 路徑不存在時,錯誤訊息要明講 tokenizer
#[test]
fn e7_qwen3_missing_tokenizer_error_mentions_tokenizer() {
    let tmp = Tmp::new();
    let dir = tmp.path().join("qwen3");
    for n in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
        touch(&dir.join(n));
    }
    let cfg = Qwen3Config { dir: dir.clone(), tokenizer: dir.join("no-such-tokenizer"), num_threads: 1, name: "qwen3-test" };
    let Err(e) = SherpaQwen3::new(&cfg) else {
        panic!("tokenizer 不存在時應回 Err");
    };
    let msg = format!("{e:#}");
    assert!(msg.contains("tokenizer"), "錯誤訊息應含 tokenizer:{msg}");
}

// E8:Cohere 沒指定語言時回錯(訊息含「語言」),有指定則原樣回傳
#[test]
fn e8_cohere_requires_explicit_language() {
    let e = cohere_lang(None).expect_err("未指定語言應回 Err");
    let msg = format!("{e:#}");
    assert!(msg.contains("語言"), "錯誤訊息應含「語言」:{msg}");
    assert_eq!(cohere_lang(Some(Lang::Ja)).unwrap(), Lang::Ja);
}

// E9:只有 Whisper 支援回修,新引擎(Qwen3、Cohere)與其他引擎都不開
#[test]
fn e9_only_whisper_supports_refine() {
    let q = AsrEngine::Qwen3 { dir: PathBuf::from("x"), tokenizer: PathBuf::from("y"), name: "qwen3-0.6b" };
    let c = AsrEngine::Cohere { dir: PathBuf::from("x") };
    let w = AsrEngine::Whisper { base_url: "http://127.0.0.1:1".to_string() };
    assert!(!refine_supported(&q), "Qwen3 不該開回修");
    assert!(!refine_supported(&c), "Cohere 不該開回修");
    assert!(!refine_supported(&AsrEngine::SenseVoice));
    assert!(!refine_supported(&AsrEngine::Routed));
    assert!(refine_supported(&w), "Whisper 應開回修");
}
