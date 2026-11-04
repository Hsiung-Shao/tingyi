//! 翻譯提示依目標語言產生;來源/目標同語言與非 Hy-MT2 模型的判斷。
#![allow(non_snake_case)]

use std::collections::HashSet;

use tingyi_core::asr::Lang;
use tingyi_core::mt::{build_prompt, effective_target, should_translate, Target, TranslateRequest};
use tingyi_core::postprocess::GlossaryEntry;

/// 括號統一成半形,避免全形或半形寫法造成誤判
fn norm(s: &str) -> String {
    s.replace('\u{ff08}', "(").replace('\u{ff09}', ")")
}

fn req(target: Target, src_lang: Lang) -> TranslateRequest {
    TranslateRequest { text: "Hello there".into(), src_lang, target, ..Default::default() }
}

// T6:提示依目標語言
#[test]
fn T6_繁中目標提示含台灣用語() {
    let p = norm(&build_prompt(&req(Target::ZhTw, Lang::En)));
    assert!(p.contains("繁体中文(台湾用语)"), "實得 {p}");
}

#[test]
fn T6_簡中目標提示用簡體且不提台灣() {
    let p = build_prompt(&req(Target::ZhCn, Lang::En));
    assert!(p.contains("简体中文"), "實得 {p}");
    assert!(!p.contains("台湾") && !p.contains("台灣"), "簡中提示不該帶台灣用語:{p}"); // 目標換了提示卻沒換會一直輸出繁體
}

#[test]
fn T6_英文目標提示含術語與上下文() {
    let mut r = req(Target::En, Lang::Ja);
    r.text = "こんにちは".into();
    r.glossary = vec![GlossaryEntry { source: "魔王".into(), target: "Demon King".into() }];
    r.context = vec![("前の文です".into(), "The previous sentence.".into())];
    let p = build_prompt(&r);
    assert!(p.contains("into English"), "實得 {p}");
    assert!(p.contains("魔王") && p.contains("Demon King"), "術語表沒進提示:{p}");
    assert!(p.contains("前の文です"), "上下文原文沒進提示:{p}");
    assert!(p.contains("こんにちは"));
}

#[test]
fn T6_日文目標提示含目標語言() {
    let p = build_prompt(&req(Target::Ja, Lang::En));
    assert!(p.contains("into Japanese"), "實得 {p}");
}

#[test]
fn T6_目標代碼互轉() {
    for (code, t) in [("zh-TW", Target::ZhTw), ("zh-CN", Target::ZhCn), ("en", Target::En), ("ja", Target::Ja), ("ko", Target::Ko)] {
        assert_eq!(Target::from_code(code), Some(t));
        assert_eq!(t.code(), code);
    }
    assert_eq!(Target::from_code("fr"), None);
    assert_eq!(Target::default(), Target::ZhTw);
}

// T7:來源與目標同語言時不翻
#[test]
fn T7_來源與目標同語言時不翻() {
    let any = HashSet::new();
    assert!(!should_translate(Lang::En, Target::En, &any));
    assert!(should_translate(Lang::Ja, Target::En, &any));
    assert!(!should_translate(Lang::Zh, Target::ZhTw, &any));
    assert!(!should_translate(Lang::Zh, Target::ZhCn, &any), "簡繁都視為中文");
    assert!(should_translate(Lang::En, Target::ZhTw, &any));
}

#[test]
fn T7_限定語言清單外不翻() {
    let allowed: HashSet<Lang> = [Lang::En, Lang::Ja].into_iter().collect();
    assert!(!should_translate(Lang::Ko, Target::ZhTw, &allowed));
    assert!(should_translate(Lang::En, Target::ZhTw, &allowed));
    assert!(should_translate(Lang::Ja, Target::ZhTw, &allowed));
}

// T8:非 Hy-MT2 模型強制中文
#[test]
fn T8_非HyMT2模型強制中文() {
    assert_eq!(effective_target("Sakura-7B-Qwen2.5-v1.0-IQ4_XS", Target::En), Target::ZhTw);
    assert_eq!(effective_target("Hy-MT2-1.8B-Q8_0", Target::En), Target::En);
    for m in ["Seed-X-PPO-7B", "LMT-60-8B", "translategemma-4b-it"] {
        assert_eq!(effective_target(m, Target::Ja), Target::ZhTw, "{m}");
    }
}
