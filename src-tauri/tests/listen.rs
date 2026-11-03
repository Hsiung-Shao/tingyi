//! 擴充觸發聽譯:開始選項組裝與開始/斷線的決策。
#![allow(non_snake_case)]

use tingyi_lib::listen::{decide_start, listen_options, should_stop_on_disconnect, Owner, StartDecision};
use tingyi_lib::settings::Prefs;

fn prefs() -> Prefs {
    Prefs {
        model: "Hy-MT2-7B-Q4_K_M".into(),
        translate: true,
        asr_engine: "routed".into(),
        lang: "ja".into(),
        speaker_id: false,
        refine: false,
        glossary: "# 專有名詞\nNVIDIA=輝達\n\nGPU\t顯卡\nCPU→處理器\n只有原文=\n=只有譯名\n".into(),
        replacements: "麥克拉=麥可拉\n刪我=\n沒有分隔符\na=b=c\n".into(),
    }
}

// U20:有偏好時各欄位對應
#[test]
fn U20_擴充觸發的開始選項() {
    let o = listen_options(Some(&prefs()), "default-model");
    assert_eq!(o.model.as_deref(), Some("Hy-MT2-7B-Q4_K_M"));
    assert_eq!(o.asr_engine.as_deref(), Some("routed"));
    assert_eq!(o.lang.as_deref(), Some("ja"));
    assert_eq!(o.speaker_id, Some(false));
    assert_eq!(o.refine, Some(false));
    assert_eq!(o.show_overlay, Some(false), "擴充觸發時不該彈出浮動字幕");
    assert_eq!(o.clear_history, Some(true));
}

#[test]
fn U20_術語表逐行解析且略過註解與缺邊的行() {
    let g = listen_options(Some(&prefs()), "m").glossary.unwrap();
    let got: Vec<(&str, &str)> = g.iter().map(|e| (e.source.as_str(), e.target.as_str())).collect();
    assert_eq!(got, vec![("NVIDIA", "輝達"), ("GPU", "顯卡"), ("CPU", "處理器")]);
}

#[test]
fn U20_取代表只切第一個分隔符且右側可空() {
    let r = listen_options(Some(&prefs()), "m").replacements.unwrap();
    let got: Vec<(&str, &str)> = r.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect();
    assert_eq!(got, vec![("麥克拉", "麥可拉"), ("刪我", ""), ("a", "b=c")]); // 全部切開會把 a=b=c 弄壞
}

#[test]
fn U20_不翻譯時不給模型() {
    let p = Prefs { translate: false, ..prefs() };
    assert_eq!(listen_options(Some(&p), "m").model, None);
}

#[test]
fn U20_語言auto或空字串視為自動偵測() {
    for l in ["auto", ""] {
        let p = Prefs { lang: l.into(), ..prefs() };
        assert_eq!(listen_options(Some(&p), "m").lang, None, "lang={l:?}");
    }
}

#[test]
fn U20_沒有偏好時用預設值() {
    let o = listen_options(None, "Hy-MT2-1.8B-Q8_0");
    assert_eq!(o.model.as_deref(), Some("Hy-MT2-1.8B-Q8_0"));
    assert_eq!(o.asr_engine.as_deref(), Some("sensevoice"));
    assert_eq!(o.lang, None);
    assert_eq!(o.speaker_id, Some(true));
    assert_eq!(o.refine, Some(true));
    assert!(o.glossary.as_ref().map_or(true, |g| g.is_empty()));
    assert!(o.replacements.as_ref().map_or(true, |r| r.is_empty()));
    assert_eq!(o.show_overlay, Some(false));
    assert_eq!(o.clear_history, Some(true));
}

// U21:使用者手動開始的回合不被擴充搶走
#[test]
fn U21_使用者已手動開始時不搶() {
    assert_eq!(decide_start(Some(Owner::Manual)), StartDecision::Busy);
    assert_eq!(decide_start(None), StartDecision::Start);
    assert_eq!(decide_start(Some(Owner::Extension)), StartDecision::Attach);
}

// U22:擴充斷線只停它自己開的回合
#[test]
fn U22_擴充斷線只停它開的回合() {
    assert!(should_stop_on_disconnect(Some(Owner::Extension)));
    assert!(!should_stop_on_disconnect(Some(Owner::Manual)), "停掉使用者手動開的回合是 bug");
    assert!(!should_stop_on_disconnect(None));
}
