//! YouTube 沒字幕改用聽譯:擴充經 native host 連到本程式的 named pipe,要求聽整個瀏覽器的聲音,
//! 字幕事件(PipelineEvent)一行一則 JSON 送回去,由擴充顯示在播放器裡。
//!
//! - 使用者已在主視窗開始字幕 → 回 busy,不搶音訊來源。
//! - 擴充開的回合:用主視窗存下的開始選項,不顯示懸浮字幕;擴充斷線就停止,並還原原本的音訊來源。

use crate::settings::Prefs;
use crate::state::AppState;
use crate::StartOptions;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tingyi_core::audio::source::AudioSource;
use tingyi_core::cleanup::Replacement;
use tingyi_core::postprocess::GlossaryEntry;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

/// 目前回合是誰開的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Manual,
    Extension,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartDecision {
    Start,
    /// 已經是擴充開的回合:直接接上,不重開
    Attach,
    /// 使用者手動開的回合在跑
    Busy,
}

pub fn decide_start(running: Option<Owner>) -> StartDecision {
    match running {
        None => StartDecision::Start,
        Some(Owner::Extension) => StartDecision::Attach,
        Some(Owner::Manual) => StartDecision::Busy,
    }
}

/// 擴充斷線時只停它自己開的回合。
pub fn should_stop_on_disconnect(running: Option<Owner>) -> bool {
    running == Some(Owner::Extension)
}

/// 每行「左=右」(也接受 Tab 或 →),只切第一個分隔符;# 開頭與沒有分隔符的行略過。
fn pairs(text: &str) -> impl Iterator<Item = (String, String)> + '_ {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(|l| {
        let i = l.find(['=', '\t', '→'])?;
        let sep = l[i..].chars().next()?.len_utf8();
        Some((l[..i].trim().to_string(), l[i + sep..].trim().to_string()))
    })
}

/// 詞彙表:兩邊都要有字(與主視窗的解析相同)。
pub fn parse_glossary(text: &str) -> Vec<GlossaryEntry> {
    pairs(text).filter(|(s, t)| !s.is_empty() && !t.is_empty()).map(|(source, target)| GlossaryEntry { source, target }).collect()
}

/// 取代表:右側可以空(刪除規則)。
pub fn parse_replacements(text: &str) -> Vec<Replacement> {
    pairs(text).filter(|(f, _)| !f.is_empty()).map(|(from, to)| Replacement { from, to }).collect()
}

/// 擴充觸發時的開始選項:照主視窗存下的選項;沒存過用預設。不顯示懸浮字幕,
/// 也不清主視窗的字幕紀錄(手動開始時會先問,這裡沒人可問,所以接續)。
pub fn listen_options(prefs: Option<&Prefs>, default_model: &str) -> StartOptions {
    let mut o = match prefs {
        Some(p) => StartOptions {
            model: p.translate.then(|| p.model.clone()),
            asr_engine: Some(p.asr_engine.clone()),
            lang: (!p.lang.is_empty() && p.lang != "auto").then(|| p.lang.clone()),
            speaker_id: Some(p.speaker_id),
            refine: Some(p.refine),
            glossary: Some(parse_glossary(&p.glossary)),
            replacements: Some(parse_replacements(&p.replacements)),
            ..Default::default()
        },
        None => StartOptions {
            model: Some(default_model.to_string()),
            asr_engine: Some("sensevoice".into()),
            speaker_id: Some(true),
            refine: Some(true),
            ..Default::default()
        },
    };
    o.show_overlay = Some(false);
    o.clear_history = Some(false);
    o
}

// ───────────────────────── 程式內:named pipe 與回合 ─────────────────────────

/// 程式啟動時開 named pipe(名稱帶亂數,寫進 runtime.json 給 native host 找)。
pub fn start_server(app: &AppHandle) {
    let name = format!(r"\\.\pipe\tingyi-listen-{}", &tingyi_core::translate_server::new_api_key()[..16]);
    crate::web::set_listen_pipe(app, name.clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            // 只接本機連線;同一時間一個用戶端(擴充只讓一個分頁在聽)
            let server = match create_private_pipe(&name, first) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("建立聽譯 pipe 失敗:{e}");
                    return;
                }
            };
            first = false;
            if server.connect().await.is_ok() {
                serve_client(&app, server).await;
            }
        }
    });
}

/// 只允許目前使用者(擁有者)與 SYSTEM 連的 DACL;P = 不繼承上層權限。
/// 預設 DACL 會讓 Everyone 唯讀,本機其他使用者就能讀字幕或佔住唯一的連線。
const PIPE_SDDL: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

/// 建立只有目前使用者能連的 named pipe 實例(只接本機連線)。
pub fn create_private_pipe(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    let sddl: Vec<u16> = PIPE_SDDL.encode_utf16().chain([0]).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY:sddl 以 0 結尾;成功時 sd 由系統配置,下面以 LocalFree 釋放
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
        .map_err(std::io::Error::other)?;
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    // SAFETY:sa 與它指到的 sd 在呼叫期間有效;CreateNamedPipe 會複製安全性描述元
    let r = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, &mut sa as *mut SECURITY_ATTRIBUTES as *mut std::ffi::c_void)
    };
    // SAFETY:sd 由 ConvertStringSecurityDescriptorToSecurityDescriptorW 配置
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
    }
    r
}

/// pipe 目前的 DACL(SDDL 字串),診斷與測試用。
pub fn pipe_dacl_sddl(pipe: &NamedPipeServer) -> std::io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY:handle 在 pipe 存活期間有效;成功時 sd 由系統配置,下面釋放
    let err = unsafe {
        GetSecurityInfo(HANDLE(pipe.as_raw_handle()), SE_KERNEL_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, Some(&mut sd))
    };
    if err.is_err() {
        return Err(std::io::Error::from_raw_os_error(err.0 as i32));
    }
    let mut out = PWSTR::null();
    // SAFETY:sd 由 GetSecurityInfo 配置;out 成功時由系統配置
    let r = unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut out, None) };
    let text = r.map_err(std::io::Error::other).and_then(|_| unsafe { out.to_string() }.map_err(std::io::Error::other));
    // SAFETY:兩者都由系統以 LocalAlloc 配置
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if !out.is_null() {
            let _ = LocalFree(Some(HLOCAL(out.0 as *mut std::ffi::c_void)));
        }
    }
    text
}

async fn write_line(w: &mut (impl AsyncWriteExt + Unpin), v: &str) -> std::io::Result<()> {
    w.write_all(v.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await
}

fn status(state: &str, message: Option<String>) -> String {
    json!({ "type": "listen_status", "state": state, "message": message }).to_string()
}

async fn serve_client(app: &AppHandle, pipe: NamedPipeServer) {
    let (r, mut w) = tokio::io::split(pipe);
    let mut lines = BufReader::new(r).lines();
    let st = app.state::<AppState>();
    // 開始前就訂閱,開始後的第一批事件才不會漏掉
    let mut rx = st.live.0.subscribe();
    let mut attached = false;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                match v["type"].as_str() {
                    Some("start") => {
                        let _ = write_line(&mut w, &status("starting", None)).await;
                        let (state, msg) = on_start(app, &v).await;
                        attached = state == "running";
                        // 沒開成(使用中、錯誤)就關掉連線,不留著佔用
                        if write_line(&mut w, &status(state, msg)).await.is_err() || !attached {
                            break;
                        }
                    }
                    Some("stop") => break,
                    _ => {}
                }
            }
            ev = rx.recv(), if attached => match ev {
                Ok(line) => {
                    // 回合結束(例如主視窗按停止)也關掉連線:host 隨之結束,擴充才能重新開始
                    if write_line(&mut w, &line).await.is_err() || line == ENDED {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }
    // 擴充斷線或要求停止:只停它自己開的回合。停止要數秒(關管線、切回網頁模型),
    // 交給另一個 task,下一個 pipe 實例才能馬上建立,換分頁的 host 不會連不上
    let owner = *st.owner.lock().unwrap();
    if should_stop_on_disconnect(owner) {
        // 停止期間新連線進來要等它停完,不能接上這個即將結束的回合
        st.stopping.store(true, std::sync::atomic::Ordering::SeqCst);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let st = app.state::<AppState>();
            if let Err(e) = crate::commands::stop_inner(app.clone(), &st).await {
                tracing::warn!("停止擴充開的回合失敗:{e}");
            }
            st.stopping.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
}

const ENDED: &str = r#"{"type":"ended"}"#;

/// 處理開始要求;回 (listen_status 的 state, message)。
async fn on_start(app: &AppHandle, v: &Value) -> (&'static str, Option<String>) {
    let st = app.state::<AppState>();
    // 上一個擴充回合還在停止(換分頁、停了馬上再開):等它停完再開新的
    for _ in 0..150 {
        if !st.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    // 另一個回合正在啟動(還沒有歸屬):當成使用中,不去改音訊來源
    if st.starting.load(std::sync::atomic::Ordering::SeqCst) {
        return ("busy", Some("聽譯正在啟動另一個回合".into()));
    }
    let owner = *st.owner.lock().unwrap();
    match decide_start(owner) {
        StartDecision::Busy => ("busy", Some("聽譯正在使用中(主視窗已開始字幕)".into())),
        StartDecision::Attach => ("running", None),
        StartDecision::Start => {
            let Ok(browser) = serde_json::from_value::<AudioSource>(v["browser"].clone()) else {
                return ("error", Some("沒有收到瀏覽器程序".into()));
            };
            let prev = st.audio_source.get().0;
            st.audio_source.set_waiting(false);
            st.audio_source.set(browser);
            let prefs = crate::settings::load(&crate::web::settings_file()).prefs;
            let opts = listen_options(prefs.as_ref(), &crate::commands::default_mt_model());
            match crate::commands::start_inner(app.clone(), &st, opts).await {
                Ok(()) => {
                    *st.listen_prev_source.lock().unwrap() = Some(prev);
                    *st.owner.lock().unwrap() = Some(Owner::Extension);
                    ("running", None)
                }
                Err(e) => {
                    st.audio_source.set(prev);
                    ("error", Some(e))
                }
            }
        }
    }
}

/// 回合停止後:清掉歸屬;擴充開的回合通知擴充結束並還原原本的音訊來源。
pub(crate) fn after_stop(state: &AppState) {
    let owner = state.owner.lock().unwrap().take();
    if owner == Some(Owner::Extension) {
        let _ = state.live.0.send(ENDED.to_string());
        if let Some(prev) = state.listen_prev_source.lock().unwrap().take() {
            state.audio_source.set(prev);
        }
    }
}
