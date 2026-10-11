//! 辨識紀錄匯出:把前端的句子列表排成純文字對照檔(原文、譯文、時間、說話者、修/補標記)。

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// 前端送來的一句(欄位對應 `src/lib/types.ts` 的 `Sentence`)。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExportSentence {
    pub id: u64,
    pub source: String,
    pub translation: String,
    /// transcribed | translating | done | failed | no_translate
    pub state: String,
    pub lang: String,
    pub speaker: Option<String>,
    /// 在串流中的起點(毫秒,原點 = 按下開始)。
    pub start_ms: Option<u64>,
    #[serde(default)]
    pub revised: bool,
    #[serde(default)]
    pub recovered: bool,
}

/// 毫秒 → `hh:mm:ss`。
pub fn format_timestamp(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// 一句的標頭行:`[時間] 說話者 · 語言 · 標記`(缺的部分省略)。
fn header(s: &ExportSentence) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(sp) = s.speaker.as_deref().filter(|x| !x.trim().is_empty()) {
        parts.push(sp.to_string());
    }
    parts.push(s.lang.to_uppercase());
    if s.revised {
        parts.push("回修".into());
    }
    if s.recovered {
        parts.push("補回".into());
    }
    let rest = parts.join(" · ");
    match s.start_ms {
        Some(ms) => format!("[{}] {rest}", format_timestamp(ms)),
        None => rest,
    }
}

/// 排成純文字對照。`exported_at` 由呼叫端提供(本機時間字串),方便測試。
pub fn to_text(sentences: &[ExportSentence], exported_at: &str) -> String {
    let mut out = format!("聽譯 辨識紀錄\n匯出時間:{exported_at}\n共 {} 句\n", sentences.len());
    for s in sentences {
        out.push('\n');
        out.push_str(&header(s));
        out.push('\n');
        out.push_str(&format!("原文:{}\n", s.source));
        match s.state.as_str() {
            "no_translate" => {}
            "done" => out.push_str(&format!("譯文:{}\n", s.translation)),
            "failed" => out.push_str("譯文:(翻譯失敗)\n"),
            _ => out.push_str("譯文:(尚未完成)\n"),
        }
    }
    out
}

/// 寫成 UTF-8(帶 BOM,避免舊工具當成 ANSI 開成亂碼)。
pub fn write_text_file(path: &Path, text: &str) -> Result<()> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(path, bytes).with_context(|| format!("寫入 {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(source: &str, state: &str, lang: &str) -> ExportSentence {
        ExportSentence {
            source: source.to_string(),
            state: state.to_string(),
            lang: lang.to_string(),
            ..Default::default()
        }
    }

    /// 取得某句「原文:」前一行(標頭行)。
    fn header_of(text: &str, source: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let idx = lines
            .iter()
            .position(|l| *l == format!("原文:{source}"))
            .expect("找不到該句原文行");
        assert!(idx > 0, "原文行前應有標頭行");
        lines[idx - 1].to_string()
    }

    // E1 毫秒轉時分秒
    #[test]
    fn format_timestamp_converts_ms() {
        assert_eq!(format_timestamp(0), "00:00:00", "0 毫秒應為 00:00:00");
        assert_eq!(format_timestamp(61_000), "00:01:01", "61 秒應為 00:01:01");
        assert_eq!(format_timestamp(3_723_000), "01:02:03", "3723 秒應為 01:02:03");
    }

    // E2 一般句輸出原文與譯文
    #[test]
    fn done_sentence_has_source_and_translation() {
        let mut s = sent("X 原文", "done", "ja");
        s.translation = "Y 譯文".to_string();
        s.start_ms = Some(83_000);
        let t = to_text(&[s], "t");
        assert!(t.contains("[00:01:23]"), "應含時間 [00:01:23]");
        assert!(t.contains("原文:X 原文"), "應含原文行");
        assert!(t.contains("譯文:Y 譯文"), "應含譯文行");
    }

    // E3 不翻譯句只有原文
    #[test]
    fn no_translate_sentence_has_no_translation_line() {
        let t = to_text(&[sent("只有原文", "no_translate", "ja")], "t");
        assert!(t.contains("原文:只有原文"), "應有原文行");
        assert!(!t.contains("譯文:"), "不翻譯句不應有譯文行");
    }

    // E4 失敗與未完成有標示
    #[test]
    fn failed_and_pending_are_marked() {
        let t = to_text(
            &[sent("甲", "failed", "ja"), sent("乙", "translating", "ja")],
            "t",
        );
        assert!(t.contains("譯文:(翻譯失敗)"), "失敗句應標示翻譯失敗");
        assert!(t.contains("譯文:(尚未完成)"), "翻譯中句應標示尚未完成");
    }

    // E5 回修與補回有標記
    #[test]
    fn revised_and_recovered_are_marked() {
        let mut a = sent("回修句", "done", "ja");
        a.revised = true;
        let mut b = sent("補回句", "done", "ja");
        b.recovered = true;
        let t = to_text(&[a, b], "t");
        let ha = header_of(&t, "回修句");
        let hb = header_of(&t, "補回句");
        assert!(ha.contains("回修"), "回修句標頭應含回修:{ha}");
        assert!(!ha.contains("補回"), "回修句標頭不應含補回:{ha}");
        assert!(hb.contains("補回"), "補回句標頭應含補回:{hb}");
        assert!(!hb.contains("回修"), "補回句標頭不應含回修:{hb}");
    }

    // E6 說話者與語言標頭
    #[test]
    fn header_has_speaker_and_uppercase_lang() {
        let mut a = sent("有人說", "done", "ja");
        a.speaker = Some("說話者 1".to_string());
        a.start_ms = Some(0);
        let mut b = sent("無人說", "done", "en");
        b.start_ms = Some(0);
        let t = to_text(&[a, b], "t");
        assert_eq!(header_of(&t, "有人說"), "[00:00:00] 說話者 1 · JA", "有說話者標頭格式");
        assert_eq!(header_of(&t, "無人說"), "[00:00:00] EN", "無說話者標頭格式");
    }

    // E7 缺時間不印時間
    #[test]
    fn missing_start_ms_omits_timestamp() {
        let t = to_text(&[sent("無時間", "done", "ja")], "t");
        let h = header_of(&t, "無時間");
        assert_eq!(h, "JA", "缺時間時標頭只剩語言");
        assert!(!h.contains('['), "不應印時間括號");
    }

    // E8 檔頭含匯出時間與句數
    #[test]
    fn file_header_has_time_and_count() {
        let ss = vec![
            sent("一", "done", "ja"),
            sent("二", "done", "ja"),
            sent("三", "done", "ja"),
        ];
        let t = to_text(&ss, "2026-10-11 14:03");
        let lines: Vec<&str> = t.lines().take(3).collect();
        assert_eq!(
            lines,
            vec!["聽譯 辨識紀錄", "匯出時間:2026-10-11 14:03", "共 3 句"],
            "檔頭三行內容與順序"
        );
    }

    // E9 寫檔帶 BOM 可讀回
    #[test]
    fn write_text_file_has_bom_and_utf8() {
        let path = std::env::temp_dir()
            .join(format!("tingyi-export-test-{}.txt", std::process::id()));
        let r = write_text_file(&path, "甲乙\n丙");
        let bytes = std::fs::read(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        assert!(r.is_ok(), "寫檔應成功");
        assert!(bytes.len() >= 3, "檔案應至少含 BOM");
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "開頭應為 UTF-8 BOM");
        assert_eq!(&bytes[3..], "甲乙\n丙".as_bytes(), "BOM 後應為原文 UTF-8");
    }
}
