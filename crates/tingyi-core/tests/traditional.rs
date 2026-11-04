//! 譯文後處理的簡轉繁:繁體字幕不可被改字或換詞。
#![allow(non_snake_case)]

use tingyi_core::postprocess::{postprocess, to_taiwan_terms};

const TRADITIONAL: [&str; 6] = [
    "干擾訊號很強",
    "鄰里之間",
    "以下項目請參考文件",
    "我支持你的決定",
    "台灣的平台",
    "這次優化了速度",
];

// Z5:已是台灣繁體的字幕原樣
#[test]
fn Z5_已是台灣繁體的字幕不被改字或換詞() {
    // 逐字轉換會把「干擾」「鄰里」「優化」等改壞;一次列出全部被改的句子
    let mut changed = Vec::new();
    for t in TRADITIONAL {
        let a = to_taiwan_terms(t);
        if a != t {
            changed.push(format!("to_taiwan_terms({t}) = {a}"));
        }
        let b = postprocess(t, &[]);
        if b != t {
            changed.push(format!("postprocess({t}) = {b}"));
        }
    }
    assert!(changed.is_empty(), "繁體句被改:{changed:#?}");
}

#[test]
fn Z5_含簡體字的字幕轉成繁體() {
    let out = to_taiwan_terms("这是软件");
    assert!(out.contains("軟體"), "實得 {out}");
    assert!(!out.contains('这') && !out.contains('软'), "實得 {out}");
    assert!(postprocess("这是软件", &[]).contains("軟體"));
}

// Z6:依小句各自判斷簡繁;內建詞表不改台灣常用詞
#[test]
fn Z6_簡繁混雜時只轉簡體的小句() {
    assert_eq!(to_taiwan_terms("今天天气很好。明天會下雨。"), "今天天氣很好。明天會下雨。");
}

#[test]
fn Z6_內建詞表不改台灣常用詞() {
    // 詞表把「計算機」「大數據」換成別的詞會改壞台灣用語;一次列出全部被改的句子
    let changed: Vec<String> = ["用計算機算一下", "大數據分析"]
        .iter()
        .filter(|t| to_taiwan_terms(t) != **t)
        .map(|t| format!("{t} -> {}", to_taiwan_terms(t)))
        .collect();
    assert!(changed.is_empty(), "台灣用語被改:{changed:?}");
}
