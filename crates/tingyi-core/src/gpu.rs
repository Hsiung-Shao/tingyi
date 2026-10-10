//! 顯示記憶體取樣(`nvidia-smi`);無 NVIDIA 顯示卡時回 None。

use std::process::Command;
use std::sync::OnceLock;

/// 執行 `nvidia-smi` 並回傳 stdout;失敗回 None。
///
/// Windows 正式版是 GUI 子系統、沒有主控台,子程序不帶 `CREATE_NO_WINDOW` 會每次新開一個
/// 主控台視窗(閃一下就關、還會搶焦點)。取樣器每 2 秒呼叫一次,所以一定要帶。
fn nvidia_smi(args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 目前整張卡已用的顯示記憶體(MiB)。
pub fn nvidia_used_mib() -> Option<u32> {
    nvidia_smi(&["--query-gpu=memory.used", "--format=csv,noheader,nounits", "--id=0"])?
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// 一次取齊:GPU 使用率 %、已用 / 總顯示記憶體 MiB、溫度 °C。
#[derive(Debug, Clone, Copy, serde::Serialize, Default)]
pub struct GpuSample {
    pub util_pct: u32,
    pub mem_used_mib: u32,
    pub mem_total_mib: u32,
    pub temp_c: u32,
}

pub fn nvidia_sample() -> Option<GpuSample> {
    let s = nvidia_smi(&[
        "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu",
        "--format=csv,noheader,nounits",
        "--id=0",
    ])?;
    let mut it = s.lines().next()?.split(',').map(|x| x.trim().parse::<u32>().unwrap_or(0));
    Some(GpuSample {
        util_pct: it.next()?,
        mem_used_mib: it.next()?,
        mem_total_mib: it.next()?,
        temp_c: it.next().unwrap_or(0),
    })
}

/// 顯示卡名稱與總量(MiB)。執行期間不會變,只查一次後快取。
pub fn nvidia_info() -> Option<(String, u32)> {
    static INFO: OnceLock<Option<(String, u32)>> = OnceLock::new();
    INFO.get_or_init(|| {
        let s = nvidia_smi(&["--query-gpu=name,memory.total", "--format=csv,noheader,nounits", "--id=0"])?;
        let line = s.lines().next()?;
        let mut it = line.split(',');
        let name = it.next()?.trim().to_string();
        let total = it.next()?.trim().parse().ok()?;
        Some((name, total))
    })
    .clone()
}
