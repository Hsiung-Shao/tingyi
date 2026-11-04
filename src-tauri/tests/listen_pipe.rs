//! 聽譯 named pipe 的存取權限:只給目前使用者(與 SYSTEM)。
#![allow(non_snake_case)]

use tingyi_lib::listen::{create_private_pipe, pipe_dacl_sddl};

fn unique_name() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!(r"\\.\pipe\tingyi-test-{}-{nanos}", std::process::id())
}

/// 取出 SDDL 內所有「允許」ACE 的受託人欄位
fn allow_trustees(sddl: &str) -> Vec<String> {
    sddl.split('(')
        .skip(1)
        .filter_map(|ace| {
            let ace = ace.split(')').next()?;
            let f: Vec<&str> = ace.split(';').collect();
            (f.first() == Some(&"A") || f.first() == Some(&"OA")).then(|| f.last().unwrap_or(&"").to_string())
        })
        .collect()
}

// P3:DACL 不含 Everyone / Anonymous / Users 的允許項,本人仍能連
#[tokio::test]
async fn P3_聽譯pipe不開放給所有人() {
    let name = unique_name();
    let server = create_private_pipe(&name, true).expect("建立 pipe 失敗");

    let sddl = pipe_dacl_sddl(&server).expect("讀 DACL 失敗");
    assert!(sddl.contains("(A;"), "至少要有一個允許項,實得 {sddl}");
    let trustees = allow_trustees(&sddl);
    assert!(!trustees.is_empty(), "解析不到允許項:{sddl}");
    for banned in ["WD", "AN", "BU", "S-1-1-0", "S-1-5-7", "S-1-5-32-545"] {
        assert!(!trustees.iter().any(|t| t == banned), "DACL 不該允許 {banned}:{sddl}"); // 預設 DACL 會放行 Everyone 讀取
    }

    let client = std::fs::OpenOptions::new().read(true).write(true).open(&name);
    assert!(client.is_ok(), "目前使用者應能連上自己的 pipe:{:?}", client.err());
}
