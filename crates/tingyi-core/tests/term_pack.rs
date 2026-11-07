//! 遊戲術語包:命中比對、誤聽修正、熱詞提示、合併術語表、載入壞檔。
#![allow(non_snake_case)]

use tingyi_core::postprocess::GlossaryEntry;
use tingyi_core::terms::{merge_glossary, Alias, TermEntry, TermPack, HOTWORD_MAX_CHARS};

fn entry(en: &str, zh: &str) -> TermEntry {
    TermEntry {
        en: en.into(),
        zh: zh.into(),
        src: String::new(),
        count: 1,
        alt: Vec::new(),
    }
}

fn pack(entries: Vec<TermEntry>) -> TermPack {
    TermPack { entries, ..Default::default() }
}

fn ens(hits: &[&TermEntry]) -> Vec<String> {
    hits.iter().map(|e| e.en.clone()).collect()
}

// T1:不分大小寫與複數可命中,黏在一起的詞不命中
#[test]
fn T1_大小寫與複數命中_黏字不命中() {
    let p = pack(vec![entry("Chaos Orb", "混沌石")]);
    let h = p.hits("I found chaos orbs", 20);
    assert_eq!(ens(&h), vec!["Chaos Orb"]);
    assert!(p.hits("chaosorb is", 20).is_empty());
}

// T2:特殊複數(X of Y、f→ves、所有格)都能命中
#[test]
fn T2_特殊複數與所有格命中() {
    let p = pack(vec![
        entry("Orb of Alteration", "改造石"),
        entry("Quarterstaff", "長棍"),
        entry("Kitava", "奇塔弗"),
    ]);
    assert_eq!(ens(&p.hits("two Orbs of Alteration", 20)), vec!["Orb of Alteration"]);
    assert_eq!(ens(&p.hits("Quarterstaves are good", 20)), vec!["Quarterstaff"]);
    assert_eq!(ens(&p.hits("Kitava's realm", 20)), vec!["Kitava"]);
}

// T3:長詞優先且不重疊,內含的短詞不重複命中
#[test]
fn T3_長詞優先不重疊() {
    let p = pack(vec![entry("Orb of Alteration", "改造石"), entry("Alteration", "改造")]);
    assert_eq!(ens(&p.hits("use an Orb of Alteration", 20)), vec!["Orb of Alteration"]);
}

// T4:命中數超過上限時只回 max 個
#[test]
fn T4_命中數受上限限制() {
    let mut entries = Vec::new();
    let mut words = Vec::new();
    for i in 0..30u8 {
        let suffix: String = [(b'a' + i / 26) as char, (b'a' + i % 26) as char].iter().collect();
        let en = format!("Vault{suffix}");
        words.push(en.clone());
        entries.push(entry(&en, "庫"));
    }
    let p = pack(entries);
    let text = words.join(" ");
    assert_eq!(p.hits(&text, 20).len(), 20);
}

// T5:合併術語表時使用者條目在前,與使用者同原文的包條目被略過
#[test]
fn T5_合併術語表使用者優先() {
    let user = vec![GlossaryEntry { source: "chaos orb".into(), target: "自訂".into() }];
    let p = pack(vec![entry("Chaos Orb", "混沌石"), entry("Kitava", "奇塔弗")]);
    let hits = p.hits("Chaos Orb and Kitava", 20);
    assert_eq!(hits.len(), 2, "前提:兩個都命中");
    let merged = merge_glossary(user, &hits);
    let pairs: Vec<(String, String)> =
        merged.iter().map(|g| (g.source.clone(), g.target.clone())).collect();
    assert_eq!(
        pairs,
        vec![
            ("chaos orb".to_string(), "自訂".to_string()),
            ("Kitava".to_string(), "奇塔弗".to_string())
        ]
    );
}

fn alias_pack() -> TermPack {
    TermPack {
        aliases: vec![
            Alias { from: "Path of Excel".into(), to: "Path of Exile".into() },
            Alias { from: "Isamite".into(), to: "Ezomyte".into() },
        ],
        ..Default::default()
    }
}

// T6:誤聽修正不分大小寫、整字比對
#[test]
fn T6_誤聽修正整字不分大小寫() {
    let p = alias_pack();
    assert_eq!(p.correct_aliases("welcome to path of excel"), "welcome to Path of Exile");
    assert_eq!(p.correct_aliases("excellent path"), "excellent path");
    assert_eq!(p.correct_aliases("ISAMITE armour"), "Ezomyte armour");
}

// T7:熱詞提示只含熱詞、不超長、不截半個詞
#[test]
fn T7_熱詞提示不含誤聽詞且不超長() {
    let hotwords: Vec<String> = (0..30).map(|i| format!("Hotword{:013}", i)).collect();
    assert!(hotwords.iter().all(|h| h.chars().count() == 20), "前提:每個熱詞 20 字元");
    let p = TermPack {
        hotwords: hotwords.clone(),
        aliases: vec![Alias { from: "Isamite".into(), to: "Ezomyte".into() }],
        ..Default::default()
    };
    let prompt = p.hotword_prompt();
    assert!(!prompt.is_empty());
    assert!(!prompt.contains("Isamite"));
    assert!(prompt.chars().count() <= HOTWORD_MAX_CHARS);
    for seg in prompt.split(", ") {
        assert!(hotwords.iter().any(|h| h == seg), "出現不完整的詞:{seg}");
    }
    assert!(prompt.starts_with(&hotwords[0]), "應保留前面的詞");
}

// T8:壞 JSON 載入回 Err,錯誤訊息含檔名
#[test]
fn T8_壞檔載入失敗且訊息含檔名() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken-pack.json");
    std::fs::write(&path, "{ 這不是 json").unwrap();
    let err = TermPack::load(&path).expect_err("壞檔應回 Err");
    let msg = format!("{err:#}");
    assert!(msg.contains("broken-pack.json"), "訊息未含檔名:{msg}");
}
