//! 音訊來源:系統全部聲音,或指定程式(process loopback,含子程序)。
//!
//! - `SourceControl`:目前來源 + 世代號;使用者切換或程式重開重綁時遞增,擷取執行緒看到就重建 session。
//! - 程式清單:各輸出裝置的音訊 session → pid → 往上找同名最上層程序(瀏覽器由 audio service
//!   子程序發聲,要擷取整棵樹)→ 依最上層 pid 去重。
//! - 目標程式結束時 process loopback 不會報錯、只會一直送靜音(實測),所以要自己輪詢 pid,
//!   並以 start_time 防 pid 被重用。

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use wasapi::{DeviceEnumerator, Direction, SessionState};

const WEBVIEW_EXE: &str = "msedgewebview2.exe";

/// 擷取來源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioSource {
    /// 預設輸出裝置的全部聲音。
    #[default]
    System,
    /// 指定程式(最上層 pid,含整棵程序樹)。
    App { pid: u32, exe: String, start_time: u64 },
}

/// 一個程序的資訊(列舉與重綁用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    /// 執行檔名,例如 `chrome.exe`。
    pub exe: String,
    /// 啟動時間(秒,epoch);與 pid 一起判斷是不是同一個程序。
    pub start_time: u64,
}

pub type ProcTable = HashMap<u32, ProcInfo>;

/// 一個音訊 session 的觀察值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionObs {
    pub pid: u32,
    /// 至少有一條串流正在跑。
    pub active: bool,
    pub display_name: String,
}

/// 清單上的一個程式(已依最上層 pid 去重)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppAudio {
    pub pid: u32,
    pub exe: String,
    /// 顯示名稱:session 有名字就用,否則用去掉 `.exe` 的執行檔名。
    pub name: String,
    /// 任一 session 正在播放。
    pub active: bool,
    pub start_time: u64,
}

fn exe_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// 父程序;父 pid 已被比自己晚啟動的程序重用(Windows 不清父 pid)時視為沒有父。
fn parent_of<'a>(p: &ProcInfo, table: &'a ProcTable) -> Option<&'a ProcInfo> {
    let parent = table.get(&p.parent?)?;
    (parent.start_time <= p.start_time).then_some(parent)
}

/// 往上找同執行檔名(不分大小寫)的最上層祖先;`msedgewebview2.exe` 往上找到宿主程式為止。
/// pid 不在表中時回傳自己。
pub fn find_root(pid: u32, table: &ProcTable) -> u32 {
    let Some(start) = table.get(&pid) else { return pid };
    let webview = exe_eq(&start.exe, WEBVIEW_EXE);
    let mut cur = start;
    let mut seen = HashSet::from([pid]);
    while let Some(parent) = parent_of(cur, table) {
        if !seen.insert(parent.pid) {
            break;
        }
        if webview && !exe_eq(&parent.exe, WEBVIEW_EXE) {
            return find_root(parent.pid, table);
        }
        if !webview && !exe_eq(&parent.exe, &start.exe) {
            break;
        }
        cur = parent;
    }
    cur.pid
}

/// `pid` 是否為 `ancestor` 本身或其子孫。
fn in_tree(pid: u32, ancestor: u32, table: &ProcTable) -> bool {
    if pid == ancestor {
        return true;
    }
    let Some(mut cur) = table.get(&pid) else { return false };
    let mut seen = HashSet::from([pid]);
    while let Some(parent) = parent_of(cur, table) {
        if parent.pid == ancestor {
            return true;
        }
        if !seen.insert(parent.pid) {
            return false;
        }
        cur = parent;
    }
    false
}

fn exe_stem(exe: &str) -> &str {
    if exe.len() > 4 && exe[exe.len() - 4..].eq_ignore_ascii_case(".exe") {
        &exe[..exe.len() - 4]
    } else {
        exe
    }
}

/// session 觀察值 → 依最上層 pid 去重的清單;排除 pid 0(系統音效)與 `self_pid` 整棵程序樹。
/// 播放中的排前面,其次依名稱。
pub fn group_sessions(sessions: &[SessionObs], table: &ProcTable, self_pid: u32) -> Vec<AppAudio> {
    let mut out: Vec<AppAudio> = Vec::new();
    for s in sessions {
        if s.pid == 0 || in_tree(s.pid, self_pid, table) {
            continue;
        }
        let root = find_root(s.pid, table);
        if in_tree(root, self_pid, table) {
            continue;
        }
        let Some(info) = table.get(&root) else { continue };
        match out.iter_mut().find(|a| a.pid == root) {
            Some(a) => {
                a.active |= s.active;
                if !s.display_name.is_empty() && a.name == exe_stem(&a.exe) {
                    a.name = s.display_name.clone();
                }
            }
            None => out.push(AppAudio {
                pid: root,
                exe: info.exe.clone(),
                name: if s.display_name.is_empty() { exe_stem(&info.exe).to_string() } else { s.display_name.clone() },
                active: s.active,
                start_time: info.start_time,
            }),
        }
    }
    out.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// 綁定的程式是否仍是同一個程序(pid 存在且 start_time 相同)。
pub fn still_alive(pid: u32, start_time: u64, table: &ProcTable) -> bool {
    table.get(&pid).is_some_and(|p| p.start_time == start_time)
}

/// 依執行檔名找新的最上層程序:有音訊 session 的優先,其次最早啟動的;找不到回 None。
/// 回傳 (pid, start_time)。
pub fn rebind(exe: &str, table: &ProcTable, sessions: &[SessionObs]) -> Option<(u32, u64)> {
    let roots: BTreeSet<u32> = table
        .values()
        .filter(|p| exe_eq(&p.exe, exe))
        .map(|p| find_root(p.pid, table))
        .collect();
    let earliest = |cands: &mut dyn Iterator<Item = u32>| cands.min_by_key(|r| (table[r].start_time, *r));
    let with_session = earliest(&mut roots.iter().copied().filter(|r| sessions.iter().any(|s| find_root(s.pid, table) == *r)));
    let pid = with_session.or_else(|| earliest(&mut roots.iter().copied()))?;
    Some((pid, table[&pid].start_time))
}

/// 目前所有程序的快照。
pub fn snapshot_processes() -> ProcTable {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .iter()
        .map(|(pid, p)| {
            let pid = pid.as_u32();
            (
                pid,
                ProcInfo {
                    pid,
                    parent: p.parent().map(|x| x.as_u32()),
                    exe: p.name().to_string_lossy().into_owned(),
                    start_time: p.start_time(),
                },
            )
        })
        .collect()
}

/// 所有輸出裝置上未過期的音訊 session。呼叫端執行緒要能用 COM(內部會 initialize_mta)。
pub fn enumerate_sessions() -> Result<Vec<SessionObs>> {
    let _ = wasapi::initialize_mta();
    let enumerator = DeviceEnumerator::new().map_err(|e| anyhow!("DeviceEnumerator: {e}"))?;
    let devices = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|e| anyhow!("render devices: {e}"))?;
    let mut out = Vec::new();
    for dev in &devices {
        // 單一裝置失敗(拔除中、驅動異常)不影響其他裝置
        let r = (|| -> Result<()> {
            let dev = dev.map_err(|e| anyhow!("device: {e}"))?;
            let sessions = dev
                .get_iaudiosessionmanager()
                .and_then(|m| m.get_audiosessionenumerator())
                .map_err(|e| anyhow!("session enumerator: {e}"))?;
            for i in 0..sessions.get_count().map_err(|e| anyhow!("get_count: {e}"))? {
                let Ok(c) = sessions.get_session(i) else { continue };
                let Ok(state) = c.get_state() else { continue };
                if state == SessionState::Expired {
                    continue;
                }
                let Ok(pid) = c.get_process_id() else { continue };
                out.push(SessionObs {
                    pid,
                    active: state == SessionState::Active,
                    display_name: c.get_display_name().unwrap_or_default(),
                });
            }
            Ok(())
        })();
        if let Err(e) = r {
            tracing::debug!("略過一個輸出裝置的 session 列舉:{e:#}");
        }
    }
    Ok(out)
}

/// 目前有音訊 session 的程式清單(排除本程式)。
pub fn list_app_audio() -> Result<Vec<AppAudio>> {
    let sessions = enumerate_sessions()?;
    Ok(group_sessions(&sessions, &snapshot_processes(), std::process::id()))
}

/// 擷取執行緒用:重用同一個 `System` 輪詢目標程式是否還活著。
pub struct ProcWatch {
    sys: System,
}

impl Default for ProcWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcWatch {
    pub fn new() -> Self {
        Self { sys: System::new() }
    }

    /// pid 存在且 start_time 相同(防 pid 被重用)。
    pub fn alive(&mut self, pid: u32, start_time: u64) -> bool {
        let p = Pid::from_u32(pid);
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::nothing());
        self.sys.process(p).is_some_and(|x| x.start_time() == start_time)
    }

    /// 目標結束後依執行檔名找新的程序。
    pub fn rebind(&mut self, exe: &str) -> Option<(u32, u64)> {
        let sessions = enumerate_sessions().unwrap_or_default();
        rebind(exe, &snapshot_processes(), &sessions)
    }
}

/// 目前來源 + 世代號 + 擷取狀態;Tauri 與擷取執行緒共用同一個 `Arc`。
#[derive(Debug, Default)]
pub struct SourceControl {
    /// 來源與世代號放同一把鎖:分開讀可能拿到「舊來源 + 新世代號」而漏掉切換。
    state: Mutex<(AudioSource, u64)>,
    /// 世代號的鏡像,擷取迴圈每輪比對用(不必拿鎖)。
    gen: AtomicU64,
    /// 指定程式已結束、正在等它重新開啟。
    waiting: AtomicBool,
}

impl SourceControl {
    pub fn new(source: AudioSource) -> Self {
        Self {
            state: Mutex::new((source, 0)),
            ..Default::default()
        }
    }

    /// 目前來源與世代號。
    pub fn get(&self) -> (AudioSource, u64) {
        self.state.lock().unwrap().clone()
    }

    pub fn gen(&self) -> u64 {
        self.gen.load(Ordering::SeqCst)
    }

    /// 換來源並遞增世代號,回傳新世代號。
    pub fn set(&self, source: AudioSource) -> u64 {
        let mut st = self.state.lock().unwrap();
        st.0 = source;
        st.1 += 1;
        self.gen.store(st.1, Ordering::SeqCst);
        st.1
    }

    pub fn waiting(&self) -> bool {
        self.waiting.load(Ordering::SeqCst)
    }

    pub fn set_waiting(&self, waiting: bool) {
        self.waiting.store(waiting, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: Option<u32>, exe: &str, start: u64) -> ProcInfo {
        ProcInfo { pid, parent, exe: exe.to_string(), start_time: start }
    }

    fn table(items: Vec<ProcInfo>) -> ProcTable {
        items.into_iter().map(|i| (i.pid, i)).collect()
    }

    fn obs(pid: u32, active: bool, name: &str) -> SessionObs {
        SessionObs { pid, active, display_name: name.to_string() }
    }

    // T10:瀏覽器孫程序回最上層同名 pid
    #[test]
    fn find_root_chrome_grandchild_returns_top_chrome() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(10, Some(1), "chrome.exe", 200),
            p(11, Some(10), "chrome.exe", 201),
            p(12, Some(11), "Chrome.exe", 202),
        ]);
        assert_eq!(find_root(12, &t), 10);
        assert_eq!(find_root(11, &t), 10);
        assert_eq!(find_root(10, &t), 10);
    }

    // T10:webview2 子程序回宿主程式 pid
    #[test]
    fn find_root_webview_returns_host_pid() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(20, Some(1), "foo.exe", 200),
            p(21, Some(20), "msedgewebview2.exe", 201),
            p(22, Some(21), "msedgewebview2.exe", 202),
        ]);
        assert_eq!(find_root(22, &t), 20);
        assert_eq!(find_root(21, &t), 20);
    }

    // T10:不在表中的 pid 回自己
    #[test]
    fn find_root_unknown_pid_returns_self() {
        let t = table(vec![p(1, None, "explorer.exe", 100)]);
        assert_eq!(find_root(9999, &t), 9999);
        assert_eq!(find_root(1, &t), 1);
    }

    // T10:父子循環不會無窮迴圈且回傳循環內的 pid
    #[test]
    fn find_root_cycle_terminates() {
        let t = table(vec![
            p(30, Some(31), "chrome.exe", 100),
            p(31, Some(30), "chrome.exe", 101),
        ]);
        let r = find_root(30, &t);
        assert!(r == 30 || r == 31, "循環應回循環內某 pid,實得 {r}");
    }

    // T11:同程式多 session 合併、排除 pid 0 與本程式樹、保留 active
    #[test]
    fn group_sessions_merges_and_filters() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(10, Some(1), "chrome.exe", 200),
            p(11, Some(10), "chrome.exe", 201),
            p(12, Some(10), "chrome.exe", 202),
            p(50, Some(1), "tingyi.exe", 300),
            p(51, Some(50), "msedgewebview2.exe", 301),
            p(60, Some(1), "spotify.exe", 400),
        ]);
        let sessions = vec![
            obs(11, false, ""),
            obs(0, true, "System Sounds"),
            obs(12, true, ""),
            obs(50, true, ""),
            obs(51, true, ""),
            obs(60, false, ""),
        ];
        let out = group_sessions(&sessions, &t, 50);
        assert_eq!(out.len(), 2, "應只剩 chrome 與 spotify,實得 {out:?}");
        assert!(out.iter().all(|a| a.pid != 0), "不得含 pid 0");
        assert!(out.iter().all(|a| a.pid != 50 && a.pid != 51), "不得含本程式及子孫");

        let chrome = out.iter().find(|a| a.exe == "chrome.exe").expect("缺 chrome");
        assert_eq!(chrome.pid, 10);
        assert!(chrome.active, "任一 session 播放中即 active");
        assert_eq!(chrome.name, "chrome");
        assert_eq!(chrome.start_time, 200);

        let spotify = out.iter().find(|a| a.exe == "spotify.exe").expect("缺 spotify");
        assert_eq!(spotify.pid, 60);
        assert!(!spotify.active);
        assert_eq!(spotify.name, "spotify");
        assert_eq!(spotify.start_time, 400);
    }

    // T11:有 display_name 時 name 用 display_name
    #[test]
    fn group_sessions_uses_display_name_when_present() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(60, Some(1), "spotify.exe", 400),
        ]);
        let out = group_sessions(&[obs(60, true, "Spotify Premium")], &t, 9999);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Spotify Premium");
        assert_eq!(out[0].exe, "spotify.exe");
        assert_eq!(out[0].pid, 60);
    }

    // T12:pid 與 start_time 都相符才算同一程序
    #[test]
    fn still_alive_checks_start_time() {
        let t = table(vec![p(100, Some(1), "chrome.exe", 2000)]);
        assert!(still_alive(100, 2000, &t));
        assert!(!still_alive(100, 1000, &t), "start_time 不同代表 pid 被重用");
        assert!(!still_alive(999, 1, &t), "pid 不在表中");
    }

    // T13:重綁優先選有 session 的那棵樹,大小寫不分
    #[test]
    fn rebind_prefers_tree_with_session() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(400, Some(1), "chrome.exe", 1000),
            p(500, Some(1), "Chrome.exe", 2000),
            p(501, Some(500), "Chrome.exe", 2001),
        ]);
        let sessions = vec![obs(501, true, "")];
        assert_eq!(rebind("chrome.exe", &t, &sessions), Some((500, 2000)));
        assert_eq!(rebind("CHROME.EXE", &t, &sessions), Some((500, 2000)));
    }

    // T13:都沒 session 時選最早啟動的最上層程序
    #[test]
    fn rebind_without_session_picks_earliest_root() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(410, Some(1), "chrome.exe", 1500),
            p(400, Some(1), "chrome.exe", 1000),
            p(401, Some(400), "chrome.exe", 1001),
        ]);
        assert_eq!(rebind("chrome.exe", &t, &[]), Some((400, 1000)));
    }

    // T13:表中沒有該程式時回 None
    #[test]
    fn rebind_none_when_no_such_exe() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(60, Some(1), "spotify.exe", 400),
        ]);
        assert_eq!(rebind("chrome.exe", &t, &[obs(60, true, "")]), None);
    }

    // T14:每次 set 世代號加一,get 回最後設定的來源
    #[test]
    fn source_control_set_bumps_generation() {
        let c = SourceControl::new(AudioSource::System);
        let g0 = c.gen();
        let app = AudioSource::App { pid: 10, exe: "chrome.exe".into(), start_time: 200 };
        let g1 = c.set(app);
        assert_eq!(g1, g0 + 1);
        let g2 = c.set(AudioSource::System);
        assert_eq!(g2, g1 + 1);
        assert_eq!(c.gen(), g0 + 2);
        let (src, gen) = c.get();
        assert_eq!(src, AudioSource::System);
        assert_eq!(gen, g0 + 2);
    }

    // T14:set 為 App 後 get 回該 App 來源
    #[test]
    fn source_control_get_returns_last_set_app() {
        let c = SourceControl::new(AudioSource::System);
        let app = AudioSource::App { pid: 10, exe: "chrome.exe".into(), start_time: 200 };
        c.set(app.clone());
        let (src, gen) = c.get();
        assert_eq!(src, app);
        assert_eq!(gen, c.gen());
    }
}
