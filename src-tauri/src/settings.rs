//! `settings.json`:跨次啟動保留的設定。擴充在背景喚起聽譯時主視窗還是預設狀態,
//! 網頁翻譯的模型與主視窗的開始選項都要從這裡讀。

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// 網頁翻譯預設的模型(合併前擴充一直用它,這台機器上比 7B 快約 10 倍)。
pub const DEFAULT_WEB_MODEL: &str = "Hy-MT2-1.8B-Q8_0";

/// 主視窗的開始選項。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Prefs {
    pub model: String,
    pub translate: bool,
    pub asr_engine: String,
    pub lang: String,
    pub speaker_id: bool,
    pub refine: bool,
    /// 詞彙表原文(每行 原文=譯名)
    pub glossary: String,
    /// 辨識取代表原文(每行 錯字=正字)
    pub replacements: String,
    /// 字幕譯成哪種語言(zh-TW / zh-CN / en / ja / ko;空 = zh-TW)
    pub target_lang: String,
    /// 簡體譯文怎麼轉(terms / script / none;空 = terms)
    pub zh_convert: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub web_model: String,
    /// 主視窗還沒存過就是 None
    pub prefs: Option<Prefs>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { web_model: DEFAULT_WEB_MODEL.into(), prefs: None }
    }
}

/// 讀不到或格式壞掉就用預設
pub fn load(path: &Path) -> Settings {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 先寫同目錄暫存檔再改名,不會留下半份檔
pub fn save(path: &Path, s: &Settings) -> io::Result<()> {
    let body = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
