//! 譯文後處理:OpenCC s2twp 簡轉繁(含台灣用語)、內建詞表補充、使用者術語表(最高優先)。
//!
//! OpenCC 用純 Rust 的 `opencc-fmmseg`(MIT,字典內嵌)。只轉簡體譯文(Sakura 只會輸出簡體、Hy-MT2 偶爾):
//! s2twp 套在已是台灣繁體的句子上會改錯字與換掉台灣原本就用的詞(干擾→幹擾、項目→專案)。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

fn opencc() -> &'static opencc_fmmseg::OpenCC {
    static C: OnceLock<opencc_fmmseg::OpenCC> = OnceLock::new();
    C.get_or_init(opencc_fmmseg::OpenCC::new)
}

/// 一條術語:原文(大小寫不敏感比對)→ 指定譯法。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlossaryEntry {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Glossary {
    pub entries: Vec<GlossaryEntry>,
}

impl Glossary {
    pub fn new(pairs: &[(&str, &str)]) -> Self {
        Self {
            entries: pairs
                .iter()
                .map(|(s, t)| GlossaryEntry {
                    source: s.to_string(),
                    target: t.to_string(),
                })
                .collect(),
        }
    }

    /// 本句原文命中的術語(給 Hy-MT2 術語提示用)。
    pub fn hits<'a>(&'a self, source_text: &str) -> Vec<&'a GlossaryEntry> {
        let lower = source_text.to_lowercase();
        self.entries
            .iter()
            .filter(|e| !e.source.is_empty() && lower.contains(&e.source.to_lowercase()))
            .collect()
    }
}

/// 常見大陸用詞 → 台灣用詞(只收歧義低、字幕中常見者;長詞優先避免局部替換)。
const TW_TERMS: &[(&str, &str)] = &[
    ("視頻", "影片"),
    ("信息", "資訊"),
    ("軟件", "軟體"),
    ("硬件", "硬體"),
    ("網絡", "網路"),
    ("服務器", "伺服器"),
    ("用戶", "使用者"),
    ("鼠標", "滑鼠"),
    ("屏幕", "螢幕"),
    ("博客", "部落格"),
    ("打印", "列印"),
    ("內存", "記憶體"),
    ("默認", "預設"),
    ("激光", "雷射"),
    ("操作系統", "作業系統"),
    ("互聯網", "網際網路"),
    ("智能", "智慧"),
    ("搜索", "搜尋"),
    ("創建", "建立"),
];

/// 這段是不是簡體:轉成簡體不變、轉成繁體會變。已是繁體(或繁簡同形)的句子不算。
fn is_simplified(text: &str) -> bool {
    let cc = opencc();
    cc.convert(text, "t2s", false) == text && cc.convert(text, "s2t", false) != text
}

/// 小句的結尾標點:簡繁混雜時依小句各自判斷,簡體的小句才轉。
const CLAUSE_END: &[char] = &['。', '！', '？', '；', '，', '!', '?', ';', ',', '\n'];

/// 簡體譯文轉台灣繁體與用語(OpenCC s2twp,不動標點);再套內建詞表補繁體字形的大陸用詞(視頻→影片)。
pub fn to_taiwan_terms(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for clause in text.split_inclusive(CLAUSE_END) {
        if is_simplified(clause) {
            out.push_str(&opencc().convert(clause, "s2twp", false));
        } else {
            out.push_str(clause);
        }
    }
    for (from, to) in TW_TERMS {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// 完整後處理:先做台灣用語,再套術語表(術語表優先,最後執行以免被覆蓋)。
pub fn postprocess(translation: &str, glossary_hits: &[&GlossaryEntry]) -> String {
    let mut out = to_taiwan_terms(translation);
    for hit in glossary_hits {
        // 譯文中若模型沒照術語翻,無法可靠定位;只做「原文直接殘留」的替換。
        if !hit.source.is_empty() && out.contains(hit.source.as_str()) {
            out = out.replace(hit.source.as_str(), hit.target.as_str());
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplified_to_traditional() {
        let out = to_taiwan_terms("这是一个简体测试，软件很好用。");
        assert!(out.contains("這是一個簡體測試"), "{out}");
        assert!(out.contains("軟體"), "{out}");
        // 已是繁體的字幕不應被改壞
        assert_eq!(to_taiwan_terms("歡迎回來,今天要挑戰新地圖。"), "歡迎回來,今天要挑戰新地圖。");
    }

    #[test]
    fn taiwan_terms_replace() {
        assert_eq!(to_taiwan_terms("這個視頻的信息很多"), "這個影片的資訊很多");
        assert_eq!(to_taiwan_terms("操作系統與軟件"), "作業系統與軟體");
        assert_eq!(to_taiwan_terms("沒有需要改的"), "沒有需要改的");
    }

    #[test]
    fn glossary_hits_case_insensitive() {
        let g = Glossary::new(&[("Exile", "流亡者"), ("Wraeclast", "瓦爾克拉斯")]);
        let hits = g.hits("Welcome to wraeclast, exile.");
        assert_eq!(hits.len(), 2);
        assert!(g.hits("nothing here").is_empty());
    }

    #[test]
    fn postprocess_applies_glossary_last() {
        let g = Glossary::new(&[("Wraeclast", "瓦爾克拉斯")]);
        let hits = g.hits("Wraeclast");
        assert_eq!(postprocess(" 歡迎來到 Wraeclast 的視頻 ", &hits), "歡迎來到 瓦爾克拉斯 的影片");
    }
}
