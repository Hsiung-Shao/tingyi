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
    // 用 get 避免切在多位元組字元中間而 panic
    match exe.len().checked_sub(4).filter(|&n| n > 0).and_then(|n| exe.get(n..).map(|tail| (n, tail))) {
        Some((n, tail)) if tail.eq_ignore_ascii_case(".exe") => &exe[..n],
        _ => exe,
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

const FRAME_HOST_EXE: &str = "ApplicationFrameHost.exe";

/// 一個頂層視窗的原始屬性(Win32 列舉結果;過濾與歸屬交給純函式,方便測試)。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub visible: bool,
    /// 有 owner(對話框、浮動面板)。
    pub owned: bool,
    /// `WS_EX_TOOLWINDOW`。
    pub tool_window: bool,
    /// DWM 隱藏(其他虛擬桌面、背景 UWP)。
    pub cloaked: bool,
    /// 子視窗所屬的 pid(只對 ApplicationFrameHost 收集,UWP 的實際程序在子視窗)。
    pub child_pids: Vec<u32>,
}

/// 清單分組。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceGroup {
    /// 程式樹有正在播放的 session。
    Playing,
    /// 有視窗但沒在播放。
    Windowed,
    /// 只有音訊 session、沒有視窗。
    Background,
}

/// 清單項目的種類。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// 一個可見視窗(同程式多視窗各一項,擷取的都是整個程式)。
    Window { hwnd: isize, title: String },
    /// 沒有視窗的程式;名稱規則同 `group_sessions`。
    Background { name: String },
}

/// 音訊來源清單的一項;選它就是擷取 `pid` 的整棵程序樹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceEntry {
    #[serde(flatten)]
    pub kind: SourceKind,
    pub group: SourceGroup,
    /// 最上層程序。
    pub pid: u32,
    pub exe: String,
    pub start_time: u64,
    pub active: bool,
}

/// 使用者看得到的頂層視窗:可見、有標題、沒有 owner、不是工具視窗、沒被 DWM 隱藏,且不是桌面。
pub fn keep_window(w: &RawWindow) -> bool {
    w.visible
        && !w.title.trim().is_empty()
        && !w.owned
        && !w.tool_window
        && !w.cloaked
        && !matches!(w.class.as_str(), "Progman" | "WorkerW")
}

/// 視窗所屬程序;ApplicationFrameHost 的視窗改用第一個不同 pid 的子視窗(UWP 實際程序)。
pub fn window_pid(w: &RawWindow, table: &ProcTable) -> u32 {
    let frame_host = table.get(&w.pid).is_some_and(|p| exe_eq(&p.exe, FRAME_HOST_EXE));
    if frame_host {
        w.child_pids.iter().copied().find(|&c| c != w.pid).unwrap_or(w.pid)
    } else {
        w.pid
    }
}

/// 視窗(依 Z 順序,最上層在前)+ 音訊 session → 清單。內部套用 `keep_window`、`window_pid`、`find_root`;
/// 排除 pid 0 與 `self_pid` 整棵樹。順序:播放中(視窗依 Z 順序,其後沒有視窗的程式依名稱)、
/// 有視窗的程式(依 exe、再依標題)、背景程式(依名稱)。
pub fn build_source_list(windows: &[RawWindow], sessions: &[SessionObs], table: &ProcTable, self_pid: u32) -> Vec<SourceEntry> {
    let excluded = |pid: u32| pid == 0 || in_tree(pid, self_pid, table);
    let playing_pids: Vec<u32> = sessions.iter().filter(|s| s.active && !excluded(s.pid)).map(|s| s.pid).collect();
    // 擷取的是整棵程序樹:子孫(即使執行檔名不同,例如 LINE 的 LineCall)在播放也算這個視窗在播放
    let tree_playing = |root: u32| playing_pids.iter().any(|&p| in_tree(p, root, table));

    let (mut playing, mut windowed) = (Vec::new(), Vec::new());
    let mut window_roots = HashSet::new();
    for w in windows.iter().filter(|w| keep_window(w)) {
        let pid = window_pid(w, table);
        let root = find_root(pid, table);
        if excluded(pid) || excluded(root) {
            continue;
        }
        let Some(info) = table.get(&root) else { continue };
        window_roots.insert(root);
        let active = tree_playing(root);
        let entry = SourceEntry {
            kind: SourceKind::Window { hwnd: w.hwnd, title: w.title.clone() },
            group: if active { SourceGroup::Playing } else { SourceGroup::Windowed },
            pid: root,
            exe: info.exe.clone(),
            start_time: info.start_time,
            active,
        };
        if active {
            playing.push(entry);
        } else {
            windowed.push(entry);
        }
    }
    let title = |e: &SourceEntry| match &e.kind {
        SourceKind::Window { title, .. } => title.to_lowercase(),
        SourceKind::Background { name } => name.to_lowercase(),
    };
    windowed.sort_by(|a, b| a.exe.to_lowercase().cmp(&b.exe.to_lowercase()).then_with(|| title(a).cmp(&title(b))));

    // 沒有視窗的程式:沿用 group_sessions 的去重與命名(已依播放中、名稱排序)
    let mut background = Vec::new();
    for a in group_sessions(sessions, table, self_pid) {
        if window_roots.contains(&a.pid) {
            continue;
        }
        let entry = SourceEntry {
            kind: SourceKind::Background { name: a.name },
            group: if a.active { SourceGroup::Playing } else { SourceGroup::Background },
            pid: a.pid,
            exe: a.exe,
            start_time: a.start_time,
            active: a.active,
        };
        if a.active {
            playing.push(entry);
        } else {
            background.push(entry);
        }
    }
    playing.extend(windowed);
    playing.extend(background);
    playing
}

/// 音訊來源清單:可見視窗 + 音訊 session(排除本程式)。
pub fn list_audio_sources() -> Result<Vec<SourceEntry>> {
    let sessions = enumerate_sessions()?;
    let table = snapshot_processes();
    let frame_hosts: HashSet<u32> = table.values().filter(|p| exe_eq(&p.exe, FRAME_HOST_EXE)).map(|p| p.pid).collect();
    let windows = super::window_list::enumerate_windows(&frame_hosts);
    Ok(build_source_list(&windows, &sessions, &table, std::process::id()))
}

/// 綁定的程式是否仍是同一個程序(pid 存在且 start_time 相同)。
pub fn still_alive(pid: u32, start_time: u64, table: &ProcTable) -> bool {
    table.get(&pid).is_some_and(|p| p.start_time == start_time)
}

/// 依執行檔名找新的最上層程序:有音訊 session 的優先,其次最早啟動的;找不到回 None。
/// 候選 root 的執行檔名必須與 `exe` 相同(webview2 的宿主不同名,不會被選),且不在 `self_pid` 程序樹內。
/// 回傳 (pid, start_time)。
pub fn rebind(exe: &str, table: &ProcTable, sessions: &[SessionObs], self_pid: u32) -> Option<(u32, u64)> {
    let roots: BTreeSet<u32> = table
        .values()
        .filter(|p| exe_eq(&p.exe, exe))
        .map(|p| find_root(p.pid, table))
        .filter(|r| table.get(r).is_some_and(|i| exe_eq(&i.exe, exe)) && !in_tree(*r, self_pid, table))
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
        rebind(exe, &snapshot_processes(), &sessions, std::process::id())
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

    /// 世代號仍是 `expected` 時才換來源並遞增,回傳新世代號;已被別人改過則不寫,回 None。
    pub fn set_if_gen(&self, expected: u64, source: AudioSource) -> Option<u64> {
        let mut st = self.state.lock().unwrap();
        if st.1 != expected {
            return None;
        }
        st.0 = source;
        st.1 += 1;
        self.gen.store(st.1, Ordering::SeqCst);
        Some(st.1)
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
        assert_eq!(rebind("chrome.exe", &t, &sessions, 9999), Some((500, 2000)));
        assert_eq!(rebind("CHROME.EXE", &t, &sessions, 9999), Some((500, 2000)));
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
        assert_eq!(rebind("chrome.exe", &t, &[], 9999), Some((400, 1000)));
    }

    // T13:表中沒有該程式時回 None
    #[test]
    fn rebind_none_when_no_such_exe() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(60, Some(1), "spotify.exe", 400),
        ]);
        assert_eq!(rebind("chrome.exe", &t, &[obs(60, true, "")], 9999), None);
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

    // set_if_gen:世代相符才寫入並遞增,不符回 None 且來源與世代不變
    #[test]
    fn set_if_gen_writes_only_when_generation_matches() {
        let c = SourceControl::new(AudioSource::System);
        let (_, g0) = c.get();
        let app = AudioSource::App { pid: 10, exe: "chrome.exe".into(), start_time: 200 };
        assert_eq!(c.set_if_gen(g0, app.clone()), Some(g0 + 1));
        assert_eq!(c.get(), (app.clone(), g0 + 1));
        assert_eq!(c.gen(), g0 + 1);
        // 過期世代:不得蓋掉
        assert_eq!(c.set_if_gen(g0, AudioSource::System), None);
        assert_eq!(c.get(), (app, g0 + 1));
        assert_eq!(c.gen(), g0 + 1);
    }

    // exe_stem:非 ASCII、大小寫、過短檔名都不 panic 且回正確值
    #[test]
    fn exe_stem_handles_non_ascii_and_edge_cases() {
        assert_eq!(exe_stem("測試"), "測試");
        assert_eq!(exe_stem("測試.exe"), "測試");
        assert_eq!(exe_stem("chrome.EXE"), "chrome");
        assert_eq!(exe_stem("a.exe"), "a");
        assert_eq!(exe_stem("exe"), "exe");
        assert_eq!(exe_stem(".exe"), ".exe");
        assert_eq!(exe_stem(""), "");
    }

    // rebind:目標為 webview2 時宿主 exe 不同,不得回傳宿主 pid
    #[test]
    fn rebind_webview_target_never_returns_host() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(20, Some(1), "foo.exe", 200),
            p(21, Some(20), "msedgewebview2.exe", 201),
        ]);
        assert_eq!(rebind("msedgewebview2.exe", &t, &[obs(21, true, "")], 9999), None);
    }

    // rebind:目標程序在 self_pid 樹內時不被選中,改選樹外的同名程序
    #[test]
    fn rebind_skips_processes_inside_self_tree() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(50, Some(1), "tingyi.exe", 150),
            p(51, Some(50), "helper.exe", 160),
            p(70, Some(1), "helper.exe", 900),
        ]);
        assert_eq!(rebind("helper.exe", &t, &[], 50), Some((70, 900)));
        let only_self = table(vec![
            p(1, None, "explorer.exe", 100),
            p(50, Some(1), "tingyi.exe", 150),
            p(51, Some(50), "helper.exe", 160),
        ]);
        assert_eq!(rebind("helper.exe", &only_self, &[], 50), None);
    }

    fn win(hwnd: isize, pid: u32, title: &str) -> RawWindow {
        RawWindow {
            hwnd,
            pid,
            title: title.to_string(),
            class: "Chrome_WidgetWin_1".to_string(),
            visible: true,
            ..Default::default()
        }
    }

    fn chrome_table() -> ProcTable {
        table(vec![
            p(1, None, "explorer.exe", 100),
            p(10, Some(1), "chrome.exe", 200),
            p(11, Some(10), "chrome.exe", 201),
            p(12, Some(10), "chrome.exe", 202),
        ])
    }

    // T1:同程式多視窗各一項且指向最上層程序
    #[test]
    fn build_source_list_multi_windows_one_entry_each() {
        let t = chrome_table();
        let wins = vec![win(100, 10, "YouTube - Google Chrome"), win(101, 11, "Gmail - Google Chrome")];
        let out = build_source_list(&wins, &[], &t, 9999);
        assert_eq!(out.len(), 2, "兩個視窗應各一項,實得 {out:?}");
        for (hwnd, title) in [(100, "YouTube - Google Chrome"), (101, "Gmail - Google Chrome")] {
            let e = out
                .iter()
                .find(|e| matches!(&e.kind, SourceKind::Window { hwnd: h, .. } if *h == hwnd))
                .unwrap_or_else(|| panic!("缺 hwnd {hwnd} 的項目:{out:?}"));
            assert_eq!(e.kind, SourceKind::Window { hwnd, title: title.to_string() });
            assert_eq!(e.pid, 10);
            assert_eq!(e.exe, "chrome.exe");
            assert_eq!(e.start_time, 200);
        }
    }

    // T2:視窗所屬程式有播放中 session 時歸播放中且依 Z 順序
    #[test]
    fn build_source_list_playing_windows_keep_z_order() {
        let t = chrome_table();
        let wins = vec![win(100, 10, "YouTube - Google Chrome"), win(101, 11, "Gmail - Google Chrome")];
        let out = build_source_list(&wins, &[obs(12, true, "")], &t, 9999);
        assert_eq!(out.len(), 2, "應有兩項,實得 {out:?}");
        assert!(out.iter().all(|e| e.group == SourceGroup::Playing && e.active), "應都為播放中且 active:{out:?}");
        let hwnds: Vec<isize> = out
            .iter()
            .map(|e| match &e.kind {
                SourceKind::Window { hwnd, .. } => *hwnd,
                other => panic!("應為視窗項,實得 {other:?}"),
            })
            .collect();
        assert_eq!(hwnds, vec![100, 101]);
    }

    // T3:有視窗沒聲音歸有視窗的程式並依 exe、標題排序
    #[test]
    fn build_source_list_windowed_sorted_by_exe_then_title() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(20, Some(1), "steam.exe", 200),
            p(30, Some(1), "LINE.exe", 300),
        ]);
        let wins = vec![win(200, 20, "好友"), win(201, 30, "LINE"), win(202, 20, "Steam")];
        let out = build_source_list(&wins, &[], &t, 9999);
        assert_eq!(out.len(), 3, "應有三項,實得 {out:?}");
        assert!(out.iter().all(|e| e.group == SourceGroup::Windowed && !e.active), "應都為有視窗且非 active:{out:?}");
        let titles: Vec<String> = out
            .iter()
            .map(|e| match &e.kind {
                SourceKind::Window { title, .. } => title.clone(),
                other => panic!("應為視窗項,實得 {other:?}"),
            })
            .collect();
        assert_eq!(titles, vec!["LINE", "Steam", "好友"]);
    }

    // T4:只有 session 沒視窗的程式歸背景,播放中的提到播放中
    #[test]
    fn build_source_list_background_apps_and_playing_first() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(40, Some(1), "RtkUWP.exe", 400),
            p(41, Some(1), "Genesis.exe", 410),
        ]);
        let sessions = vec![obs(40, false, ""), obs(41, true, "Genesis")];
        let out = build_source_list(&[], &sessions, &t, 9999);
        assert_eq!(out.len(), 2, "應有兩項,實得 {out:?}");
        assert_eq!(out[0].kind, SourceKind::Background { name: "Genesis".to_string() });
        assert_eq!(out[0].group, SourceGroup::Playing);
        assert_eq!(out[1].kind, SourceKind::Background { name: "RtkUWP".to_string() });
        assert_eq!(out[1].group, SourceGroup::Background);
    }

    // T5:排除本程式樹與系統音效
    #[test]
    fn build_source_list_excludes_self_tree_and_system_sounds() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(50, Some(1), "tingyi.exe", 300),
            p(51, Some(50), "msedgewebview2.exe", 301),
            p(60, Some(1), "spotify.exe", 400),
        ]);
        let wins = vec![win(500, 50, "聽譯 tingyi")];
        let sessions = vec![obs(51, true, ""), obs(0, true, "System Sounds"), obs(60, false, "")];
        let out = build_source_list(&wins, &sessions, &t, 50);
        assert_eq!(out.len(), 1, "應只剩 spotify,實得 {out:?}");
        assert!(out.iter().all(|e| e.pid != 0 && e.pid != 50 && e.pid != 51), "不得含 pid 0、50、51:{out:?}");
        assert_eq!(out[0].pid, 60);
        assert_eq!(out[0].group, SourceGroup::Background);
        assert!(matches!(out[0].kind, SourceKind::Background { .. }), "應為背景項:{out:?}");
    }

    // T6:視窗過濾規則,每個情境只改一個欄位
    #[test]
    fn keep_window_filters_each_rule() {
        assert!(keep_window(&win(1, 10, "正常視窗")), "正常視窗應保留");
        let mut w = win(1, 10, "x");
        w.visible = false;
        assert!(!keep_window(&w), "不可見應過濾");
        assert!(!keep_window(&win(1, 10, "")), "標題空應過濾");
        let mut w = win(1, 10, "x");
        w.owned = true;
        assert!(!keep_window(&w), "有 owner 應過濾");
        let mut w = win(1, 10, "x");
        w.tool_window = true;
        assert!(!keep_window(&w), "工具視窗應過濾");
        let mut w = win(1, 10, "x");
        w.cloaked = true;
        assert!(!keep_window(&w), "DWM 隱藏應過濾");
        let mut w = win(1, 10, "x");
        w.class = "Progman".to_string();
        assert!(!keep_window(&w), "Progman 桌面應過濾");
        let mut w = win(1, 10, "x");
        w.class = "WorkerW".to_string();
        assert!(!keep_window(&w), "WorkerW 桌面應過濾");
    }

    // T7:UWP 視窗改用實際程序
    #[test]
    fn window_pid_uwp_uses_real_process() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(60, Some(1), "ApplicationFrameHost.exe", 200),
            p(61, Some(1), "Calculator.exe", 210),
            p(70, Some(1), "foo.exe", 300),
            p(71, Some(1), "bar.exe", 310),
        ]);
        let mut frame = win(600, 60, "計算機");
        frame.child_pids = vec![60, 61];
        assert_eq!(window_pid(&frame, &t), 61, "frame host 應改用第一個不同 pid 的子視窗");
        let mut foo = win(700, 70, "foo");
        foo.child_pids = vec![71];
        assert_eq!(window_pid(&foo, &t), 70, "非 frame host 不得改用子視窗 pid");
        let mut frame_self = win(601, 60, "空殼");
        frame_self.child_pids = vec![60];
        assert_eq!(window_pid(&frame_self, &t), 60, "子視窗都同 pid 時維持原 pid");
    }

    // T8:同時有視窗與 session 的程式不重複出背景項
    #[test]
    fn build_source_list_window_and_session_no_duplicate_background() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(80, Some(1), "Discord.exe", 500),
        ]);
        let wins = vec![win(800, 80, "Discord - #一般")];
        let out = build_source_list(&wins, &[obs(80, true, "Discord")], &t, 9999);
        assert_eq!(out.len(), 1, "不得重複出背景項,實得 {out:?}");
        assert!(matches!(out[0].kind, SourceKind::Window { .. }), "應為視窗項:{out:?}");
        assert_eq!(out[0].group, SourceGroup::Playing);
    }

    // T9:不同執行檔的子程序在播放時父程式的視窗也算播放中
    #[test]
    fn build_source_list_child_exe_playing_marks_parent_window_playing() {
        let t = table(vec![
            p(1, None, "explorer.exe", 100),
            p(30, Some(1), "LINE.exe", 300),
            p(31, Some(30), "LineCall.exe", 310),
        ]);
        let wins = vec![win(300, 30, "LINE")];
        let out = build_source_list(&wins, &[obs(31, true, "")], &t, 9999);
        assert_eq!(out.len(), 2, "應有 LINE 視窗與 LineCall 背景兩項,實得 {out:?}");

        let line = out
            .iter()
            .find(|e| matches!(&e.kind, SourceKind::Window { hwnd: 300, .. }))
            .unwrap_or_else(|| panic!("缺 LINE 視窗項:{out:?}"));
        assert_eq!(line.group, SourceGroup::Playing, "子程序播放時 LINE 視窗應歸播放中");
        assert!(line.active, "LINE 視窗項應為 active");
        assert_eq!(line.pid, 30);

        let call = out
            .iter()
            .find(|e| e.kind == SourceKind::Background { name: "LineCall".to_string() })
            .unwrap_or_else(|| panic!("缺 LineCall 背景項:{out:?}"));
        assert_eq!(call.group, SourceGroup::Playing);
        assert_eq!(call.pid, 31);
    }
}
