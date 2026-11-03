//! YouTube 沒字幕改用聽譯:擴充經 native host 連到本程式的 named pipe,要求聽整個瀏覽器的聲音,
//! 字幕事件(PipelineEvent)一行一則 JSON 送回去,由擴充顯示在播放器裡。
//!
//! - 使用者已在主視窗開始字幕 → 回 busy,不搶音訊來源。
//! - 擴充開的回合:用主視窗存下的開始選項,不顯示懸浮字幕;擴充斷線就停止,並還原原本的音訊來源。

use crate::settings::Prefs;
use crate::StartOptions;
use tingyi_core::cleanup::Replacement;
use tingyi_core::postprocess::GlossaryEntry;

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

/// 擴充觸發時的開始選項:照主視窗存下的選項;沒存過用預設。不顯示懸浮字幕。
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
    o.clear_history = Some(true);
    o
}
