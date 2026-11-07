//! 術語包匯入/刪除/搜尋:import_pack、pack_exists、remove_pack、TermPack::search。
#![allow(non_snake_case)]

use std::fs;
use std::path::Path;

use tingyi_core::terms::{import_pack, pack_exists, remove_pack, TermEntry, TermPack};

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

fn write_src(dir: &Path, name: &str, game: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
    let es: Vec<serde_json::Value> = entries
        .iter()
        .map(|(en, zh)| serde_json::json!({ "en": en, "zh": zh }))
        .collect();
    let v = serde_json::json!({ "game": game, "label": "測試包", "entries": es });
    let p = dir.join(name);
    fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    p
}

fn file_count(dir: &Path) -> usize {
    fs::read_dir(dir).unwrap().count()
}

// P1:合法包匯入後檔案位元組相同,回傳 id 與條目數正確
#[test]
fn P1_匯入合法包_檔案相同且回傳資訊正確() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let src = write_src(src_dir.path(), "any.json", "poe1", &[("Chaos Orb", "混沌石"), ("Kitava", "奇塔弗")]);

    let info = import_pack(&src, dst.path()).expect("合法包應匯入成功");

    let out = dst.path().join("poe1.json");
    assert!(out.exists(), "目標目錄應出現 poe1.json");
    assert_eq!(fs::read(&out).unwrap(), fs::read(&src).unwrap());
    assert_eq!(info.id, "poe1");
    assert_eq!(info.entries, 2);
}

// P2:壞 JSON、空 entries、空 game 都失敗且目錄不留檔
#[test]
fn P2_壞檔匯入失敗且不留檔() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();

    let bad = src_dir.path().join("bad.json");
    fs::write(&bad, "{ not json").unwrap();
    assert!(import_pack(&bad, dst.path()).is_err(), "壞 JSON 應回 Err");
    assert_eq!(file_count(dst.path()), 0, "壞 JSON 後目錄應為空");

    let empty_entries = write_src(src_dir.path(), "empty.json", "poe1", &[]);
    assert!(import_pack(&empty_entries, dst.path()).is_err(), "entries 為空應回 Err");
    assert_eq!(file_count(dst.path()), 0, "空 entries 後目錄應為空");

    let empty_game = write_src(src_dir.path(), "nogame.json", "", &[("Chaos Orb", "混沌石")]);
    assert!(import_pack(&empty_game, dst.path()).is_err(), "game 為空應回 Err");
    assert_eq!(file_count(dst.path()), 0, "空 game 後目錄應為空");
}

// P3:game 含空白或路徑跳脫時拒絕,訊息提到 game 且不留檔
#[test]
fn P3_不合法game拒絕() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();

    for (i, game) in ["poe 1", "../x"].iter().enumerate() {
        let src = write_src(src_dir.path(), &format!("g{i}.json"), game, &[("Chaos Orb", "混沌石")]);
        let err = import_pack(&src, dst.path()).expect_err("不合法 game 應回 Err");
        let msg = format!("{err:#}");
        assert!(msg.contains("game"), "錯誤訊息應含 game,實際:{msg}");
        assert_eq!(file_count(dst.path()), 0, "game={game} 後目錄應為空");
    }
}

// P4:search 英文不分大小寫、中文子字串、完全相符排最前、受 limit 限制
#[test]
fn P4_搜尋比對與排序() {
    let p = pack(vec![
        entry("Chaos Orb Shard", "混沌石碎片"),
        entry("Chaos Orb", "混沌石"),
        entry("Kitava", "奇塔弗"),
    ]);

    let ens = |q: &str, n: usize| -> Vec<String> { p.search(q, n).iter().map(|e| e.en.clone()).collect() };

    let chaos = ens("chaos", 10);
    assert!(chaos.contains(&"Chaos Orb".to_string()));
    assert!(chaos.contains(&"Chaos Orb Shard".to_string()));

    assert!(!ens("混沌", 10).is_empty(), "中文子字串應命中");
    assert!(ens("KITA", 10).contains(&"Kitava".to_string()), "大寫查詢應命中");

    let exact = p.search("chaos orb", 10);
    assert_eq!(exact[0].en, "Chaos Orb", "完全相符應排最前");

    assert_eq!(p.search("chaos", 1).len(), 1, "limit=1 只回 1 筆");
}

// P5:remove_pack 只刪指定包,不合法 id 回 Err 且不刪任何檔
#[test]
fn P5_刪除指定包_不合法id拒絕() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("packs");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("poe1.json"), "{}").unwrap();
    fs::write(dir.join("keep.json"), "{}").unwrap();
    // 上一層也放一份,防止 ../keep 跳脫刪到
    fs::write(root.path().join("keep.json"), "{}").unwrap();

    remove_pack(&dir, "poe1").expect("合法 id 應刪除成功");
    assert!(!dir.join("poe1.json").exists(), "poe1.json 應已刪除");
    assert!(dir.join("keep.json").exists(), "keep.json 不應被刪");

    assert!(remove_pack(&dir, "../keep").is_err(), "不合法 id 應回 Err");
    assert!(dir.join("keep.json").exists());
    assert!(root.path().join("keep.json").exists(), "上層檔不應被刪");
}

// P6:pack_exists 對存在、不存在、不合法 id 的回傳
#[test]
fn P6_pack_exists判斷() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("poe1.json"), "{}").unwrap();

    assert!(pack_exists(dir.path(), "poe1"));
    assert!(!pack_exists(dir.path(), "poe2"));
    assert!(!pack_exists(dir.path(), "../x"));
}
