//! 本機設定檔:網頁翻譯模型與字幕偏好的讀寫。
#![allow(non_snake_case)]

use tingyi_lib::settings::{load, save, Prefs, Settings, DEFAULT_WEB_MODEL};

// U1:設定檔不存在時用預設
#[test]
fn U1_網頁翻譯預設用小模型() {
    let dir = tempfile::tempdir().unwrap();
    let s = load(&dir.path().join("settings.json"));
    assert_eq!(s.web_model, "Hy-MT2-1.8B-Q8_0");
    assert_eq!(DEFAULT_WEB_MODEL, "Hy-MT2-1.8B-Q8_0");
    assert_eq!(s.prefs, None);
}

// U2:內容壞掉不 panic,回預設
#[test]
fn U2_設定檔壞掉回預設() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    std::fs::write(&p, b"\xff\xfe not json {{{").unwrap();
    assert_eq!(load(&p), Settings::default());
}

// U3:寫入後讀回相同;缺欄位的舊檔也讀得到
#[test]
fn U3_設定寫入後讀回相同() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    let s = Settings {
        web_model: "Hy-MT2-1.8B-Q4_K_M".into(),
        prefs: Some(Prefs {
            model: "Hy-MT2-7B-Q4_K_M".into(),
            translate: true,
            asr_engine: "sensevoice".into(),
            lang: "ja".into(),
            speaker_id: true,
            refine: true,
            glossary: "foo=富".into(),
            replacements: "a=>b".into(),
        }),
    };
    save(&p, &s).unwrap();
    assert_eq!(load(&p), s, "存檔後讀回要相等");

    let q = dir.path().join("old.json");
    std::fs::write(&q, r#"{"web_model":"Hy-MT2-1.8B-Q4_K_M"}"#).unwrap();
    let old = load(&q);
    assert_eq!(old.web_model, "Hy-MT2-1.8B-Q4_K_M");
    assert_eq!(old.prefs, None, "缺 prefs 欄位時應為 None");
}
