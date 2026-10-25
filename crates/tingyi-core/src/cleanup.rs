//! 辨識結果清理:幻聽式重複壓縮 + 使用者辨識取代表。
//!
//! 正式辨識、預覽、回修改原文、回修補回句四個出口都要套同一份,
//! 否則回修會把已修好的字換回錯字。

use serde::{Deserialize, Serialize};

/// 一條辨識取代規則(錯字 → 正字),例如實況主名字、專有名詞。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

/// 套用取代表:每個位置取最長的符合規則,單趟掃描;替換後的輸出不再被其他規則比對(不連鎖)。
pub fn apply_replacements(text: &str, rules: &[Replacement]) -> String {
    let mut rules: Vec<&Replacement> = rules.iter().filter(|r| !r.from.is_empty()).collect();
    if rules.is_empty() {
        return text.to_string();
    }
    rules.sort_by_key(|r| std::cmp::Reverse(r.from.chars().count()));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while let Some(c) = rest.chars().next() {
        for r in &rules {
            if let Some(after) = rest.strip_prefix(r.from.as_str()) {
                out.push_str(&r.to);
                rest = after;
                continue 'scan;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

const MAX_UNIT: usize = 4;
const MIN_REPEATS: usize = 5;

/// 壓縮幻聽式重複:1–4 字的單位連續出現 ≥ 5 次時只保留 2 次(單位含數字者不壓)。
/// 壓縮後整段只剩那個重複單位(忽略標點空白)代表是幻聽,回 None。
pub fn collapse_repeats(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut units: Vec<&[char]> = Vec::new();
    let mut i = 0;
    'scan: while i < chars.len() {
        for l in 1..=MAX_UNIT {
            if i + l * MIN_REPEATS > chars.len() {
                break;
            }
            let unit = &chars[i..i + l];
            let mut n = 1;
            while i + (n + 1) * l <= chars.len() && &chars[i + n * l..i + (n + 1) * l] == unit {
                n += 1;
            }
            // 含數字的單位(如 100000 的 0)是金額/數量,不是幻聽,不壓縮
            if n >= MIN_REPEATS && !unit.iter().any(|c| c.is_numeric()) {
                out.extend(unit.iter().chain(unit));
                units.push(unit);
                i += n * l;
                continue 'scan;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    let alnum = |s: &mut dyn Iterator<Item = char>| s.filter(|c| c.is_alphanumeric()).collect::<String>();
    let core = alnum(&mut out.chars());
    let only_repeat = units.iter().any(|u| {
        let u = alnum(&mut u.iter().copied());
        !u.is_empty() && core == u.repeat(2)
    });
    (!only_repeat).then_some(out)
}

/// 四個出口共用:先壓縮重複,再套取代表;整段是幻聽回 None。
pub fn clean_transcript(text: &str, rules: &[Replacement]) -> Option<String> {
    collapse_repeats(text).map(|t| apply_replacements(&t, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(from: &str, to: &str) -> Replacement {
        Replacement { from: from.to_string(), to: to.to_string() }
    }

    // T18:整段只有單字重複的幻聽回 None
    #[test]
    fn collapse_repeats_pure_hallucination_is_none() {
        assert_eq!(collapse_repeats("ミミミミミミ"), None);
    }

    // T18:重複不足 5 次的詞原樣保留
    #[test]
    fn collapse_repeats_below_threshold_unchanged() {
        assert_eq!(collapse_repeats("やばいやばい"), Some("やばいやばい".to_string()));
    }

    // T18:字尾重複 6 次壓成 2 次並保留前面的字
    #[test]
    fn collapse_repeats_trailing_repeat_keeps_two() {
        assert_eq!(collapse_repeats("あはははははは"), Some("あはは".to_string()));
    }

    // T18:數字連續重複(金額、零)不壓縮,原樣輸出
    #[test]
    fn collapse_repeats_keeps_digit_runs() {
        assert_eq!(collapse_repeats("100000円"), Some("100000円".to_string()));
        assert_eq!(collapse_repeats("1000000"), Some("1000000".to_string()));
    }

    // T18:英文詞重複 6 次壓成 2 次(前面有其他詞所以不算整段幻聽)
    #[test]
    fn collapse_repeats_english_words() {
        let out = collapse_repeats("I said no no no no no no").expect("前面有其他詞,不應整段丟棄");
        assert_eq!(out.matches("no").count(), 2, "實得 {out:?}");
        assert!(out.starts_with("I said"), "實得 {out:?}");
    }

    // T18:空字串回 Some 空字串
    #[test]
    fn collapse_repeats_empty_is_some_empty() {
        assert_eq!(collapse_repeats(""), Some(String::new()));
    }

    // T19:最長規則優先且替換結果不再被其他規則比對
    #[test]
    fn apply_replacements_longest_first_no_chaining() {
        let rules = vec![rule("a", "b"), rule("b", "c"), rule("ab", "X")];
        assert_eq!(apply_replacements("ab a", &rules), "X b");
    }

    // T19:規則為空時原樣回傳
    #[test]
    fn apply_replacements_empty_rules_unchanged() {
        assert_eq!(apply_replacements("ab a", &[]), "ab a");
    }

    // T19:clean_transcript 幻聽回 None、否則先壓縮再套取代表
    #[test]
    fn clean_transcript_drops_hallucination_and_applies_rules() {
        let rules = vec![rule("a", "b"), rule("b", "c"), rule("ab", "X")];
        assert_eq!(clean_transcript("ミミミミミミ", &rules), None);
        assert_eq!(clean_transcript("ab a", &rules), Some("X b".to_string()));
    }
}
