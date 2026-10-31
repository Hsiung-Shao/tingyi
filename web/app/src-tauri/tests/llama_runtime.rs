//! llama 啟動參數與 runtime.json 原子寫入的整合測試。
#![allow(non_snake_case)]

use atx_app_lib::llama::LlamaSpec;
use atx_app_lib::runtime::{read, write_atomic, BridgeInfo, LlamaInfo, LlamaStatus, Runtime};
use std::path::PathBuf;

/// 參數列中緊接在 `flag` 後面的那個值
fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(|s| s.as_str())
}

// L1:llama-server 只聽本機、帶隨機金鑰,埠與模型路徑正確
#[test]
fn L1_llama參數只聽本機且帶隨機金鑰() {
    let model = PathBuf::from(r"C:\data\models\hy-mt2\model.gguf");
    let a = LlamaSpec::new(PathBuf::from(r"C:\data\bin\llama-server.exe"), model.clone(), 51234);
    let b = LlamaSpec::new(PathBuf::from(r"C:\data\bin\llama-server.exe"), model.clone(), 51234);
    let args = a.args();

    assert_eq!(after(&args, "--host"), Some("127.0.0.1"), "必須只聽本機,否則網頁可偷用 GPU;args={args:?}");
    assert_eq!(after(&args, "--api-key"), Some(a.api_key.as_str()), "必須帶本次產生的金鑰");
    assert_eq!(after(&args, "--port"), Some("51234"));
    assert_eq!(after(&args, "-m"), Some(model.to_str().unwrap()));
    assert!(a.api_key.len() >= 32, "金鑰至少 32 字元,實際 {} 字元", a.api_key.len());
    assert_ne!(a.api_key, b.api_key, "每次啟動的金鑰必須不同(固定值等於沒有金鑰)");
}

fn sample(pid: u32, port: u16) -> Runtime {
    Runtime {
        pid,
        version: "1.2.3".into(),
        status: LlamaStatus::Ready,
        llama: Some(LlamaInfo { port, api_key: format!("key-{port}"), model: "hy-mt2".into() }),
        bridge: Some(BridgeInfo { url: "http://127.0.0.1:47821/mcp".into(), token: "tok".into() }),
        message: Some(format!("第 {pid} 版")),
    }
}

// L2:runtime.json 原子寫入,每次寫完都能完整讀回,且不留暫存檔
#[test]
fn L2_runtime_json原子寫入且不留殘檔() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");

    let first = sample(100, 50001);
    write_atomic(&path, &first).unwrap();
    assert_eq!(read(&path), Some(first), "第一次寫入後應能完整讀回");

    let second = sample(200, 50002);
    write_atomic(&path, &second).unwrap();
    assert_eq!(read(&path), Some(second), "覆寫後應讀到新內容,不是舊的或半份");

    let names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["runtime.json".to_string()], "目錄內不該留下暫存檔");
}

// L2(欄位契約):檔案內容是 camelCase / kebab-case,native host 與擴充才讀得懂
#[test]
fn L2_runtime_json欄位命名符合契約() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.json");
    write_atomic(&path, &sample(100, 50001)).unwrap();

    assert!(path.is_file(), "寫入後 runtime.json 必須存在");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["pid"], 100);
    assert_eq!(v["status"], "ready");
    assert_eq!(v["llama"]["port"], 50001);
    assert_eq!(v["llama"]["apiKey"], "key-50001");
    assert_eq!(v["bridge"]["token"], "tok");
}
