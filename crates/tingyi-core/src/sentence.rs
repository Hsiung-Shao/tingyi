//! 句子完整度判斷:辨識出的一段文字「說完了沒」,決定要立刻翻、還是等下一段合併。
//!
//! VAD 只看聲音,分不出停頓與句尾;SenseVoice 的 ITN 又會替未完句補「。」,
//! 所以標點不可信,要看語言的句尾形態:日文的接續助詞/促音、韓文的連結語尾、英文的連接詞。

use crate::asr::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    /// 句子說完了,立刻翻。
    Complete,
    /// 明顯沒說完(接續助詞、促音、連接詞結尾、極短碎片),等下一段合併。
    Incomplete,
    /// 看不出來,短暫等待後再翻。
    Unknown,
}

const TRAIL_PUNCT: &[char] = &[
    '。', '、', ',', '.', '\u{FF0C}', '…', ' ', '!', '?', '\u{FF01}', '\u{FF1F}', '」', '』',
];

/// 日文:結尾為這些 → 沒說完。長的排前面。
const JA_INCOMPLETE: &[&str] = &[
    "けれど", "ながら", "けど", "から", "ので", "のに", "って", "たら", "れば", "つつ",
    "て", "で", "と", "が", "し", "に", "を", "は", "も", "へ", "や", "っ",
];
/// 日文:結尾為這些 → 說完了。長的排前面。
const JA_COMPLETE: &[&str] = &[
    "ましょう", "ください", "ました", "でした", "ません", "でしょ", "じゃん", "です", "ます",
    "かな", "よね", "ない", "た", "だ", "ね", "よ", "な", "わ", "ぞ", "ぜ",
];
const KO_INCOMPLETE: &[&str] = &["는데", "지만", "니까", "려고", "다가", "고", "서", "면", "며"];
const KO_COMPLETE: &[&str] = &["니다", "요", "다", "까", "죠", "네", "어", "아", "지"];
const EN_INCOMPLETE: &[&str] = &[
    "and", "but", "so", "because", "that", "to", "of", "the", "a", "an", "with", "or", "if", "when",
    "i", "you", "we", "they", "it's", "is", "are", "was",
];

/// 極短碎片:英文 ≤ 2 個詞、CJK ≤ 3 字(去標點後)。
pub fn is_tiny(text: &str, lang_is_cjk: bool) -> bool {
    let body: String = text
        .chars()
        .filter(|c| *c == ' ' || (!c.is_ascii_punctuation() && !TRAIL_PUNCT.contains(c)))
        .collect();
    if lang_is_cjk {
        body.chars().count() <= 3
    } else {
        body.split_whitespace().count() <= 2
    }
}

pub fn completeness(text: &str, lang: Lang) -> Completeness {
    let raw = text.trim();
    if raw.is_empty() {
        return Completeness::Complete;
    }
    if is_tiny(raw, lang.is_cjk()) {
        return Completeness::Incomplete;
    }
    let last_raw = raw.chars().last().unwrap_or(' ');
    // 原始尾端是逗號/頓號 → 明顯沒完;是問號驚嘆號 → 完整(ITN 不會亂補這兩種)
    if matches!(last_raw, '、' | ',' | '\u{FF0C}') {
        return Completeness::Incomplete;
    }
    // 問號/驚嘆號是 SenseVoice 依語調補的,比「。」可信:優先視為句尾。
    if matches!(last_raw, '!' | '?' | '\u{FF01}' | '\u{FF1F}') {
        return Completeness::Complete;
    }
    let body = raw.trim_end_matches(TRAIL_PUNCT);

    match lang {
        Lang::Ja => {
            if JA_INCOMPLETE.iter().any(|s| body.ends_with(s)) {
                return Completeness::Incomplete;
            }
            if JA_COMPLETE.iter().any(|s| body.ends_with(s)) {
                return Completeness::Complete;
            }
            Completeness::Unknown
        }
        Lang::Ko => {
            if KO_INCOMPLETE.iter().any(|s| body.ends_with(s)) {
                return Completeness::Incomplete;
            }
            if KO_COMPLETE.iter().any(|s| body.ends_with(s)) {
                return Completeness::Complete;
            }
            Completeness::Unknown
        }
        Lang::En => {
            // 英文有句點就算說完(「...not doing that.」尾字 that 不該被當未完句)
            if matches!(last_raw, '.') {
                return Completeness::Complete;
            }
            let last_word = body
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase();
            if EN_INCOMPLETE.contains(&last_word.as_str()) {
                return Completeness::Incomplete;
            }
            Completeness::Unknown
        }
        Lang::Zh | Lang::Unknown => {
            if matches!(last_raw, '。' | '.') {
                Completeness::Complete
            } else {
                Completeness::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Completeness::*;

    #[test]
    fn japanese_incomplete_endings() {
        // 使用者截圖的三句
        assert_eq!(completeness("実はごミん、その店員さんが頑張っ。", Lang::Ja), Incomplete);
        assert_eq!(completeness("実力があって。", Lang::Ja), Incomplete);
        assert_eq!(completeness("湖宮は。", Lang::Ja), Incomplete);
        assert_eq!(completeness("ってくれた努力を一つの手での全て無駄にするっていう。", Lang::Ja), Unknown);
        assert_eq!(completeness("昨日のアプデで動きが重くなった気がするんだけど、", Lang::Ja), Incomplete);
    }

    #[test]
    fn japanese_complete_endings() {
        assert_eq!(completeness("実力があるのね。", Lang::Ja), Complete);
        assert_eq!(completeness("新人騎士代表は?", Lang::Ja), Complete);
        assert_eq!(completeness("続いて模範試合を行います。", Lang::Ja), Complete);
        assert_eq!(completeness("ありえないでしょ。", Lang::Ja), Complete);
        assert_eq!(completeness("うちの中学は弁当制で持っていけない場合はパンを買う", Lang::Ja), Unknown);
    }

    #[test]
    fn tiny_fragments() {
        assert_eq!(completeness("By.", Lang::En), Incomplete);
        assert_eq!(completeness("The.", Lang::En), Incomplete);
        assert_eq!(completeness("好在。", Lang::Zh), Incomplete);
        assert_eq!(completeness("そう?", Lang::Ja), Incomplete);
        assert_eq!(completeness("ありがとう。", Lang::Ja), Unknown);
    }

    #[test]
    fn english() {
        assert_eq!(completeness("See you all tomorrow.", Lang::En), Complete);
        assert_eq!(completeness("I thought you could drive.", Lang::En), Complete);
        assert_eq!(completeness("Yeah, I was coming on in a sec and", Lang::En), Incomplete);
        assert_eq!(completeness("Sometimes when I'm driving, I can be going so fast", Lang::En), Unknown);
        assert_eq!(completeness("Hold on, did that boss just one-shot me?", Lang::En), Complete);
        assert_eq!(completeness("Thank you for not doing that.", Lang::En), Complete);
        assert_eq!(completeness("I guess. Sorry, it was mentioned though, like,", Lang::En), Incomplete);
    }

    #[test]
    fn korean() {
        assert_eq!(completeness("조금만 생각을 하면서 살면", Lang::Ko), Incomplete);
        assert_eq!(completeness("훨씬 편할 거야.", Lang::Ko), Unknown);
        assert_eq!(completeness("감사합니다.", Lang::Ko), Complete);
    }
}
