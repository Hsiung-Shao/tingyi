//! CER / WER 評測:文字正規化、編輯距離、語料彙總,以及串流評測用的合成與對齊分組。
//!
//! - 正規化(參考 hayamimi eval_accuracy.py):NFKC → 去標點符號與空白;英文另轉小寫、以詞為單位。
//!   日文/中文另把漢數字轉成阿拉伯數字,兩邊一致套用,ITN 寫法差異(「二千円」vs「2000円」)不計錯。
//! - 語料錯誤率用 micro 平均:總編輯數 ÷ 總參考單位數(短句不會被放大權重)。

use crate::asr::Lang;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

/// 依語言正規化;英文回傳以單一空白分隔的小寫詞,其餘回傳去掉標點空白的字串。
pub fn normalize(text: &str, lang: Lang) -> String {
    let nfkc: String = text.nfkc().collect();
    match lang {
        Lang::En => {
            let mapped: String = nfkc
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { ' ' })
                .collect();
            mapped.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        Lang::Ko => nfkc.chars().filter(|c| c.is_alphanumeric()).collect(),
        Lang::Ja | Lang::Zh | Lang::Unknown => kanji_to_arabic(&nfkc).chars().filter(|c| c.is_alphanumeric()).collect(),
    }
}

fn kanji_digit(c: char) -> Option<u64> {
    Some(match c {
        '〇' | '零' => 0,
        '一' => 1,
        '二' => 2,
        '三' => 3,
        '四' => 4,
        '五' => 5,
        '六' => 6,
        '七' => 7,
        '八' => 8,
        '九' => 9,
        _ => return None,
    })
}

/// 十百千:乘在前面的數字上(前面沒有數字視為 1)。
fn small_unit(c: char) -> Option<u64> {
    Some(match c {
        '十' => 10,
        '百' => 100,
        '千' => 1_000,
        _ => return None,
    })
}

/// 万億兆:把目前累積的整段(< 万)乘上去。
fn big_unit(c: char) -> Option<u64> {
    Some(match c {
        '万' => 10_000,
        '億' => 100_000_000,
        '兆' => 1_000_000_000_000,
        _ => return None,
    })
}

fn is_kanji_numeral(c: char) -> bool {
    kanji_digit(c).is_some() || small_unit(c).is_some() || big_unit(c).is_some()
}

/// 解析一段由漢數字與阿拉伯數字組成的連續字元;連續數字逐位串接(「二〇二六」→2026)。
fn parse_numeral_run(run: &[char]) -> u64 {
    let (mut total, mut section, mut num, mut has_num) = (0u64, 0u64, 0u64, false);
    for &c in run {
        if let Some(d) = kanji_digit(c).or_else(|| c.to_digit(10).map(u64::from)) {
            num = num.saturating_mul(10).saturating_add(d);
            has_num = true;
        } else if let Some(u) = small_unit(c) {
            section = section.saturating_add(if has_num { num } else { 1 }.saturating_mul(u));
            (num, has_num) = (0, false);
        } else if let Some(u) = big_unit(c) {
            let v = section.saturating_add(num);
            total = total.saturating_add(if v == 0 { 1 } else { v }.saturating_mul(u));
            (section, num, has_num) = (0, 0, false);
        }
    }
    total.saturating_add(section).saturating_add(num)
}

/// 把連續的漢數字(可夾阿拉伯數字,如「3万5000」)轉成阿拉伯數字;不含漢數字的片段原樣保留。
pub fn kanji_to_arabic(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let in_run = |c: char| is_kanji_numeral(c) || c.is_ascii_digit();
        if !in_run(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && in_run(chars[i]) {
            i += 1;
        }
        let run = &chars[start..i];
        if run.iter().any(|&c| is_kanji_numeral(c)) {
            out.push_str(&parse_numeral_run(run).to_string());
        } else {
            out.extend(run);
        }
    }
    out
}

/// 編輯距離的分解(相對於參考)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EditOps {
    pub sub: usize,
    pub del: usize,
    pub ins: usize,
    /// 參考的單位數(字或詞)。
    pub ref_len: usize,
}

impl EditOps {
    pub fn errors(&self) -> usize {
        self.sub + self.del + self.ins
    }

    /// 錯誤率;參考為空時,假設也為空回 0,否則回 1。
    pub fn rate(&self) -> f64 {
        match self.ref_len {
            0 if self.errors() == 0 => 0.0,
            0 => 1.0,
            n => self.errors() as f64 / n as f64,
        }
    }

    /// 累加(語料彙總用)。
    pub fn add(&mut self, other: &EditOps) {
        self.sub += other.sub;
        self.del += other.del;
        self.ins += other.ins;
        self.ref_len += other.ref_len;
    }
}

/// Levenshtein 對齊,回傳替換/刪除/插入次數(相同總距離時優先替換)。
pub fn edit_ops<T: PartialEq>(reference: &[T], hyp: &[T]) -> EditOps {
    let (n, m) = (reference.len(), hyp.len());
    let w = m + 1;
    let mut dp = vec![0u32; (n + 1) * w];
    for j in 0..=m {
        dp[j] = j as u32;
    }
    for i in 1..=n {
        dp[i * w] = i as u32;
        for j in 1..=m {
            let diag = dp[(i - 1) * w + j - 1] + u32::from(reference[i - 1] != hyp[j - 1]);
            dp[i * w + j] = diag.min(dp[(i - 1) * w + j] + 1).min(dp[i * w + j - 1] + 1);
        }
    }
    let mut ops = EditOps { ref_len: n, ..Default::default() };
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let cur = dp[i * w + j];
        if i > 0 && j > 0 && cur == dp[(i - 1) * w + j - 1] + u32::from(reference[i - 1] != hyp[j - 1]) {
            if reference[i - 1] != hyp[j - 1] {
                ops.sub += 1;
            }
            (i, j) = (i - 1, j - 1);
        } else if i > 0 && cur == dp[(i - 1) * w + j] + 1 {
            ops.del += 1;
            i -= 1;
        } else {
            ops.ins += 1;
            j -= 1;
        }
    }
    ops
}

/// 正規化後依語言切單位(英文用詞=WER,其餘用字=CER)再算編輯距離。
pub fn score(reference: &str, hyp: &str, lang: Lang) -> EditOps {
    let (r, h) = (normalize(reference, lang), normalize(hyp, lang));
    if lang == Lang::En {
        let rw: Vec<&str> = r.split_whitespace().collect();
        let hw: Vec<&str> = h.split_whitespace().collect();
        edit_ops(&rw, &hw)
    } else {
        let rc: Vec<char> = r.chars().collect();
        let hc: Vec<char> = h.chars().collect();
        edit_ops(&rc, &hc)
    }
}

/// 固定種子的小型亂數(xorshift64*),評測要可重現、不必引入 rand。
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// [0, 1)
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// 串流評測用:把多句接成一條長音訊,句間插入固定種子的隨機間隔並疊底噪。
/// 回傳 (樣本, 每句在長音訊中的 [start, end) 樣本區間)。
pub fn build_stream(
    utts: &[Vec<f32>],
    seed: u64,
    gap_ms: (u32, u32),
    noise_dbfs: f32,
    sample_rate: u32,
) -> (Vec<f32>, Vec<(usize, usize)>) {
    let mut rng = XorShift::new(seed);
    // 均勻分布振幅 a 的 RMS 是 a/√3,換算成目標 dBFS
    let amp = 10f32.powf(noise_dbfs / 20.0) * 3f32.sqrt();
    let noise = |rng: &mut XorShift| (rng.next_f32() * 2.0 - 1.0) * amp;
    let gap = |rng: &mut XorShift| {
        let ms = gap_ms.0 as f32 + rng.next_f32() * (gap_ms.1 - gap_ms.0) as f32;
        (ms * sample_rate as f32 / 1000.0) as usize
    };
    let mut audio = Vec::new();
    let mut spans = Vec::with_capacity(utts.len());
    for u in utts {
        for _ in 0..gap(&mut rng) {
            audio.push(noise(&mut rng));
        }
        let start = audio.len();
        for &x in u {
            audio.push(x + noise(&mut rng));
        }
        spans.push((start, audio.len()));
    }
    // 尾端留一段最長間隔,讓 VAD 收得了最後一句
    for _ in 0..(gap_ms.1 as usize * sample_rate as usize / 1000) {
        audio.push(noise(&mut rng));
    }
    (audio, spans)
}

/// 參考句與辨識段依時間重疊分組後的類型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum GroupKind {
    /// 一句對一段。
    Match,
    /// 一句被切成多段。
    Split,
    /// 多句被合成一段(或多對多)。
    Merged,
    /// 參考句沒有任何辨識段(VAD 漏掉)。
    Missed,
    /// 辨識段沒有對應參考句(幻聽或雜音)。
    Inserted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlignGroup {
    /// 參考句索引(遞增)。
    pub refs: Vec<usize>,
    /// 辨識段索引(遞增)。
    pub hyps: Vec<usize>,
    pub kind: GroupKind,
}

/// 依區間重疊(重疊樣本數 ≥ `min_overlap`)建二部圖,回傳連通分量;依最早的參考或辨識位置排序。
pub fn group_alignment(refs: &[(usize, usize)], hyps: &[(usize, usize)], min_overlap: usize) -> Vec<AlignGroup> {
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != r {
            (c, parent[c]) = (parent[c], r);
        }
        r
    }
    let nr = refs.len();
    let mut parent: Vec<usize> = (0..nr + hyps.len()).collect();
    for (i, r) in refs.iter().enumerate() {
        for (j, h) in hyps.iter().enumerate() {
            let overlap = r.1.min(h.1).saturating_sub(r.0.max(h.0));
            if overlap >= min_overlap.max(1) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, nr + j));
                parent[a] = b;
            }
        }
    }
    let mut groups: Vec<(usize, AlignGroup)> = Vec::new();
    let mut root_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for node in 0..parent.len() {
        let root = find(&mut parent, node);
        let gi = *root_of.entry(root).or_insert_with(|| {
            groups.push((usize::MAX, AlignGroup { refs: vec![], hyps: vec![], kind: GroupKind::Match }));
            groups.len() - 1
        });
        let (pos, g) = &mut groups[gi];
        if node < nr {
            g.refs.push(node);
            *pos = (*pos).min(refs[node].0);
        } else {
            g.hyps.push(node - nr);
            *pos = (*pos).min(hyps[node - nr].0);
        }
    }
    groups.sort_by_key(|(pos, _)| *pos);
    groups
        .into_iter()
        .map(|(_, mut g)| {
            g.kind = match (g.refs.len(), g.hyps.len()) {
                (0, _) => GroupKind::Inserted,
                (_, 0) => GroupKind::Missed,
                (1, 1) => GroupKind::Match,
                (1, _) => GroupKind::Split,
                _ => GroupKind::Merged,
            };
            g
        })
        .collect()
}

/// `asr-compare` 的一列:一個 VAD 段落,每個引擎一格(文字, 耗時 ms)。
pub struct CompareRow {
    pub start_ms: u64,
    pub end_ms: u64,
    pub cells: Vec<(String, u32)>,
}

/// 各引擎辨識結果並排的 markdown 表。
pub fn render_compare_table(engines: &[&str], rows: &[CompareRow]) -> String {
    let esc = |s: &str| s.replace('|', "\\|").replace(['\r', '\n'], " ");
    let mut out = format!("| 時間 (s) | {} |\n|---|{}\n", engines.join(" | "), "---|".repeat(engines.len()));
    for r in rows {
        let cells: Vec<String> = r.cells.iter().map(|(t, ms)| format!("{} ({ms} ms)", esc(t))).collect();
        out.push_str(&format!(
            "| {:.1}–{:.1} | {} |\n",
            r.start_ms as f64 / 1000.0,
            r.end_ms as f64 / 1000.0,
            cells.join(" | ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(sub: usize, del: usize, ins: usize, ref_len: usize) -> EditOps {
        EditOps { sub, del, ins, ref_len }
    }

    // T1:文字完全相同時錯誤數與錯誤率皆為 0
    #[test]
    fn score_identical_text_is_zero() {
        let r = score("今日は晴れ", "今日は晴れ", Lang::Ja);
        assert_eq!(r.errors(), 0);
        assert_eq!(r.rate(), 0.0);
        assert_eq!(r.ref_len, 5);
    }

    // T2:一替換一插入時 sub=1 ins=1 del=0
    #[test]
    fn edit_ops_sub_and_ins() {
        let r = edit_ops(&['a', 'b', 'c', 'd'], &['a', 'b', 'x', 'd', 'e']);
        assert_eq!(r, ops(1, 0, 1, 4));
    }

    // T2:只少一個字時算 1 次刪除,不算替換或插入
    #[test]
    fn edit_ops_pure_deletion() {
        let r = edit_ops(&['a', 'b', 'c'], &['a', 'c']);
        assert_eq!(r, ops(0, 1, 0, 3));
    }

    // T3:日文正規化去標點空白並把全形英數轉半形
    #[test]
    fn normalize_ja_strips_punct_and_width() {
        assert_eq!(normalize("今日は、ＡＢＣ　です。", Lang::Ja), "今日はABCです");
    }

    // T4:漢數字與阿拉伯數字寫法正規化後相等
    #[test]
    fn normalize_ja_kanji_numeral_equals_arabic() {
        let a = normalize("二千三百円", Lang::Ja);
        let b = normalize("2300円", Lang::Ja);
        assert_eq!(a, b);
        assert_eq!(a, "2300円");
    }

    // T4:kanji_to_arabic 處理單字、含〇、混合阿拉伯數字與無漢數字的片段
    #[test]
    fn kanji_to_arabic_cases() {
        assert_eq!(kanji_to_arabic("十"), "10");
        assert_eq!(kanji_to_arabic("二〇二六年"), "2026年");
        assert_eq!(kanji_to_arabic("3万5000"), "35000");
        assert_eq!(kanji_to_arabic("一緒"), "1緒");
        assert_eq!(kanji_to_arabic("abc 123"), "abc 123");
    }

    // T5:英文大小寫與標點不同但詞相同時 WER 為 0
    #[test]
    fn score_en_ignores_case_and_punct() {
        let r = score("Hello, world!", "hello world", Lang::En);
        assert_eq!(r.errors(), 0);
        assert_eq!(r.ref_len, 2);
    }

    // T5:英文少一個詞時 del=1、ref_len=3、WER 約 1/3
    #[test]
    fn score_en_word_deletion_rate() {
        let r = score("the cat sat", "the cat", Lang::En);
        assert_eq!(r.del, 1);
        assert_eq!(r.sub, 0);
        assert_eq!(r.ins, 0);
        assert_eq!(r.ref_len, 3);
        assert!((r.rate() - 1.0 / 3.0).abs() < 1e-9);
    }

    // T6:語料錯誤率為總編輯數除總字數,而非各句錯誤率平均
    #[test]
    fn corpus_rate_is_micro_average() {
        let mut total = EditOps::default();
        total.add(&ops(1, 0, 0, 2));
        total.add(&ops(2, 0, 0, 20));
        assert_eq!(total, ops(3, 0, 0, 22));
        assert!((total.rate() - 3.0 / 22.0).abs() < 1e-12);
        let macro_avg = (0.5 + 0.1) / 2.0;
        assert!((total.rate() - macro_avg).abs() > 0.1);
    }

    // T6:參考為空時,假設也空回 0、否則回 1
    #[test]
    fn rate_with_empty_reference() {
        assert_eq!(ops(0, 0, 0, 0).rate(), 0.0);
        assert_eq!(ops(0, 0, 2, 0).rate(), 1.0);
    }

    // T7:韓文空白位置不同不計錯
    #[test]
    fn score_ko_ignores_whitespace() {
        let r = score("안녕 하세요", "안녕하세요", Lang::Ko);
        assert_eq!(r.errors(), 0);
        assert_eq!(r.ref_len, 5);
    }

    // T8:串流合成回傳每句正確區間、間隔落在範圍內、同 seed 可重現
    #[test]
    fn build_stream_records_spans() {
        let sr = 16000u32;
        let utts = vec![vec![0.5f32; 1600], vec![0.5f32; 3200], vec![0.5f32; 800]];
        let (audio, spans) = build_stream(&utts, 42, (300, 1500), -60.0, sr);
        assert_eq!(spans.len(), 3);
        for (i, u) in utts.iter().enumerate() {
            assert_eq!(spans[i].1 - spans[i].0, u.len(), "第 {i} 句區間長度");
        }
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "區間不得重疊且須遞增");
            let gap_ms = (w[1].0 - w[0].1) as f64 * 1000.0 / sr as f64;
            assert!((300.0..=1500.0).contains(&gap_ms), "間隔 {gap_ms} ms 超出範圍");
        }
        assert!(audio.len() >= spans[2].1);
        for &(s, e) in &spans {
            for (k, v) in audio[s..e].iter().enumerate() {
                assert!((v - 0.5).abs() < 0.01, "區間內第 {k} 樣本 {v} 偏離 0.5");
            }
        }
    }

    // T8:同 seed 結果完全相同,換 seed 間隔不同
    #[test]
    fn build_stream_deterministic_by_seed() {
        let sr = 16000u32;
        let utts = vec![vec![0.5f32; 1600], vec![0.5f32; 3200], vec![0.5f32; 800]];
        let a = build_stream(&utts, 7, (300, 1500), -60.0, sr);
        let b = build_stream(&utts, 7, (300, 1500), -60.0, sr);
        assert_eq!(a.1.len(), 3);
        assert_eq!(a, b, "同 seed 應完全相同");
        let c = build_stream(&utts, 8, (300, 1500), -60.0, sr);
        assert_eq!(c.1.len(), 3);
        assert_ne!(a.1, c.1, "換 seed 間隔應不同");
    }

    // T9:重疊分組判出 Split、Merged、Missed、Inserted
    #[test]
    fn group_alignment_classifies_groups() {
        let refs = [(0, 1000), (2000, 3000), (4000, 5000), (6000, 7000)];
        let hyps = [(0, 400), (500, 1000), (1900, 5100), (8000, 9000)];
        let g = group_alignment(&refs, &hyps, 100);
        assert_eq!(g.len(), 4, "實得 {g:?}");
        assert_eq!(g[0], AlignGroup { refs: vec![0], hyps: vec![0, 1], kind: GroupKind::Split });
        assert_eq!(g[1], AlignGroup { refs: vec![1, 2], hyps: vec![2], kind: GroupKind::Merged });
        assert_eq!(g[2], AlignGroup { refs: vec![3], hyps: vec![], kind: GroupKind::Missed });
        assert_eq!(g[3], AlignGroup { refs: vec![], hyps: vec![3], kind: GroupKind::Inserted });
    }

    // T9:一句對一段且重疊足夠時為 Match
    #[test]
    fn group_alignment_one_to_one_is_match() {
        let g = group_alignment(&[(0, 1000)], &[(100, 900)], 100);
        assert_eq!(g, vec![AlignGroup { refs: vec![0], hyps: vec![0], kind: GroupKind::Match }]);
    }

    // T9:重疊不足 min_overlap 時不連結,各自成 Missed 與 Inserted
    #[test]
    fn group_alignment_below_min_overlap_not_linked() {
        let g = group_alignment(&[(0, 1000)], &[(950, 2000)], 100);
        assert_eq!(g.len(), 2, "實得 {g:?}");
        assert_eq!(g[0].kind, GroupKind::Missed);
        assert_eq!(g[1].kind, GroupKind::Inserted);
    }
}
