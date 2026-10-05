//! 譯文後處理:使用者術語表(最高優先)與台灣用語修正。
//!
//! 規格書原訂以 OpenCC s2twp 做保險;驗證週先以內建詞表處理最常見的大陸用詞,
//! OpenCC 整合列為 MVP 待辦(見 docs/validation-report.md 風險欄)。

use serde::{Deserialize, Serialize};

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
    ("數據", "資料"),
    ("程序", "程式"),
    ("質量", "品質"),
    ("計算機", "電腦"),
    ("操作系統", "作業系統"),
    ("移動", "行動"),
    ("互聯網", "網際網路"),
    ("智能", "智慧"),
    ("搜索", "搜尋"),
    ("創建", "建立"),
    ("支持", "支援"),
];

/// 台灣用語修正。保留給 OpenCC s2twp 取代的介面。
pub fn to_taiwan_terms(text: &str) -> String {
    let mut out = text.to_string();
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
