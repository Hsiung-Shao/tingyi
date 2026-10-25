//! 短稿補救(參考 hayamimi asr_engine.py 的 split retry):長段落辨識結果字數明顯太少時,
//! 在段內低能量處切成兩半各自重解,重解結果「更長、字速正常、保留原結果尾段」才採用。
//!
//! 這是重試不是常態路徑:hayamimi 量過無條件切段重解反而變差(FLEURS ja 8.6% → 9.9%)。

use crate::asr::Lang;

/// 段落短於此長度不補救(容不下兩句話)。
pub const MIN_SPEECH_MS: u32 = 4_000;
/// 字速下限(英數字 / 語音秒);hayamimi 以 FLEURS 校正:CJK 2.4、拉丁 6.0。
pub const DENSITY_FLOOR_CJK: f32 = 2.4;
pub const DENSITY_FLOOR_LATIN: f32 = 6.0;

/// 原結果末幾個字要在重解裡找到(hayamimi RETRY_TAIL_CHARS / RETRY_TAIL_MATCH)。
const TAIL_CHARS: usize = 12;
const TAIL_MATCH: f32 = 0.6;

fn floor(lang: Lang) -> f32 {
    if lang == Lang::En {
        DENSITY_FLOOR_LATIN
    } else {
        DENSITY_FLOOR_CJK
    }
}

/// 英數字 / 秒(標點不算,ITN 或標點還原不會改變它)。
fn density(text: &str, speech_ms: u32) -> f32 {
    text.chars().filter(|c| c.is_alphanumeric()).count() as f32 * 1000.0 / speech_ms.max(1) as f32
}

/// 是否懷疑漏字:語音長於 `MIN_SPEECH_MS` 且字速低於該語言門檻;空字串不懷疑(交給雜音處理)。
pub fn should_rescue(text: &str, speech_ms: u32, lang: Lang) -> bool {
    speech_ms > MIN_SPEECH_MS && !text.trim().is_empty() && density(text, speech_ms) < floor(lang)
}

/// 找切點(樣本索引):20 ms 一窗算能量,取低於能量中位數 20 dB 且連續 ≥ 150 ms 的最長區間中點;
/// 找不到就取 40–60% 範圍內能量最低的窗。音訊短於 2 秒回 None。
pub fn find_split(samples: &[f32], sample_rate: u32) -> Option<usize> {
    let sr = sample_rate as usize;
    if samples.len() < 2 * sr {
        return None;
    }
    let frame = sr / 50;
    let db: Vec<f32> = samples
        .chunks_exact(frame)
        .map(|f| 10.0 * (f.iter().map(|x| x * x).sum::<f32>() / frame as f32 + 1e-12).log10())
        .collect();
    let mut sorted = db.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let threshold = sorted[sorted.len() / 2] - 20.0;
    let min_run = (150 * sr / 1000).div_ceil(frame);
    // 只取段內的低能量區間:貼著開頭或結尾的靜音切下去其中一半會是空的
    let mut best: Option<(usize, usize)> = None;
    let mut run_start: Option<usize> = None;
    for (k, &e) in db.iter().enumerate() {
        match (e < threshold, run_start) {
            (true, None) => run_start = Some(k),
            (false, Some(s)) => {
                if s > 0 && k - s >= min_run && best.is_none_or(|(bs, be)| k - s > be - bs) {
                    best = Some((s, k));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some((s, e)) = best {
        return Some((s + e) * frame / 2);
    }
    let (lo, hi) = (db.len() * 2 / 5, db.len() * 3 / 5);
    let k = (lo..=hi.min(db.len() - 1)).min_by(|&a, &b| db[a].total_cmp(&db[b]))?;
    Some(k * frame + frame / 2)
}

/// 兩字串的最長共同子字串長度(字元)。
fn longest_common_substring(a: &[char], b: &[char]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut best = 0;
    for &ca in a {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            if ca == cb {
                cur[j + 1] = prev[j] + 1;
                best = best.max(cur[j + 1]);
            }
        }
        prev = cur;
    }
    best
}

/// 重解結果是否採用:比原結果長、字速回到門檻以上,且原結果末 12 字有 ≥ 60% 出現在重解中(最長共同子字串)。
pub fn accept_retry(orig: &str, retry: &str, speech_ms: u32, lang: Lang) -> bool {
    let (orig, retry): (Vec<char>, Vec<char>) = (orig.trim().chars().collect(), retry.trim().chars().collect());
    if retry.len() <= orig.len() || orig.is_empty() {
        return false;
    }
    if density(&retry.iter().collect::<String>(), speech_ms) < floor(lang) {
        return false;
    }
    let tail = &orig[orig.len().saturating_sub(TAIL_CHARS)..];
    longest_common_substring(tail, &retry) as f32 >= tail.len() as f32 * TAIL_MATCH
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, sr: u32) -> Vec<f32> {
        (0..n)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin())
            .collect()
    }

    // T20:長段落但字數極少時懷疑漏字
    #[test]
    fn should_rescue_long_speech_few_chars() {
        assert!(should_rescue("ブーバー", 6000, Lang::Ja));
    }

    // T20:字速正常的日文不補救
    #[test]
    fn should_rescue_normal_density_false() {
        let text = "今日はいい天気ですね".repeat(3);
        assert_eq!(text.chars().count(), 30);
        assert!(!should_rescue(&text, 6000, Lang::Ja));
    }

    // T20:語音短於門檻即使字少也不補救
    #[test]
    fn should_rescue_short_speech_false() {
        assert!(!should_rescue("ブーバー", 3000, Lang::Ja));
    }

    // T20:空字串不懷疑
    #[test]
    fn should_rescue_empty_text_false() {
        assert!(!should_rescue("", 6000, Lang::Ja));
    }

    // T20:英文用拉丁字速門檻,10 字母 6 秒仍偏低
    #[test]
    fn should_rescue_english_uses_latin_floor() {
        assert!(should_rescue("hello world", 6000, Lang::En));
    }

    // T21:重解更長、字速正常且保留原尾段才採用
    #[test]
    fn accept_retry_longer_with_tail_kept() {
        assert!(accept_retry("ブーバー", "シャールは12ボックスに1枚やばいやばいブーバー", 6000, Lang::Ja));
    }

    // T21:重解較長但與原結果尾段無關時不採用
    #[test]
    fn accept_retry_rejects_unrelated_longer() {
        assert!(!accept_retry("ブーバー", "今日はいい天気ですね本当にそうですね", 6000, Lang::Ja));
    }

    // T21:重解較短時不採用
    #[test]
    fn accept_retry_rejects_shorter() {
        assert!(!accept_retry("ブーバー", "ブー", 6000, Lang::Ja));
    }

    // T22:兩段正弦波中間 0.5 秒靜音時,切點落在靜音區間內
    #[test]
    fn find_split_lands_in_silence() {
        let sr = 16000u32;
        let mut s = sine(24000, sr);
        s.extend(std::iter::repeat(0.0f32).take(8000));
        s.extend(sine(32000, sr));
        let at = find_split(&s, sr).expect("應找得到切點");
        assert!((24000..32000).contains(&at), "切點 {at} 不在靜音區間 [24000, 32000)");
    }

    // T22:音訊短於 2 秒回 None
    #[test]
    fn find_split_too_short_is_none() {
        let sr = 16000u32;
        assert_eq!(find_split(&sine(16000, sr), sr), None);
    }

    // T22:無靜音時取 40%-60% 範圍內的點
    #[test]
    fn find_split_continuous_falls_back_to_middle() {
        let sr = 16000u32;
        let s = sine(64000, sr);
        let at = find_split(&s, sr).expect("連續音訊也應回切點");
        assert!((25600..=38400).contains(&at), "切點 {at} 不在 40%-60% 範圍");
    }
}
