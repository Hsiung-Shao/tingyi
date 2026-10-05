//! 顯示記憶體取樣(`nvidia-smi`);無 NVIDIA 顯示卡時回 None。

use std::process::Command;

/// 目前整張卡已用的顯示記憶體(MiB)。
pub fn nvidia_used_mib() -> Option<u32> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used",
            "--format=csv,noheader,nounits",
            "--id=0",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// 顯示卡名稱與總量(MiB)。
pub fn nvidia_info() -> Option<(String, u32)> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
            "--id=0",
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next()?;
    let mut it = line.split(',');
    let name = it.next()?.trim().to_string();
    let total = it.next()?.trim().parse().ok()?;
    Some((name, total))
}
