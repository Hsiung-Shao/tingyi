//! 辨識基本模型套件(SenseVoice、Silero VAD、說話者模型):網址、登錄、可用判斷、檔案位置。
#![allow(non_snake_case)]

use std::fs;
use std::path::{Path, PathBuf};

use tingyi_core::model_download::{package_usable, spec, ModelSpec, BASE_PACKAGES, DEFAULT_BASE_URL, SENSEVOICE, SILERO_VAD, SPEAKER};
use tingyi_core::paths::{find_model_file, SENSEVOICE_DIR_NAME};

const SPEAKER_FILE: &str = "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";

fn touch(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"x").unwrap();
}

// B1:套件登錄的網址與 SHA256 要和下載腳本同一條目一致
#[test]
fn B1_基本模型的網址與SHA256與下載腳本一致() {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/download-models.ps1");
    let script = fs::read_to_string(&script_path).unwrap_or_else(|e| panic!("讀不到 {}:{e}", script_path.display()));
    let lower = script.to_lowercase();

    let mut problems = Vec::new();
    for m in [&SENSEVOICE, &SILERO_VAD, &SPEAKER] {
        for f in m.files {
            let url = m.url(DEFAULT_BASE_URL, f);
            let Some(at) = script.find(&url) else {
                problems.push(format!("{}:腳本沒有網址 {url}", m.id));
                continue;
            };
            // 同一個條目:網址之後、下一個條目開頭之前要有同一個 SHA256
            let rest = &lower[at..];
            let entry = &rest[..rest[1..].find("@{").map_or(rest.len(), |i| i + 1)];
            if !entry.contains(&f.sha256.to_lowercase()) {
                problems.push(format!("{}:{} 的 SHA256 {} 不在腳本同一條目", m.id, f.name, f.sha256)); // 任一邊改版沒同步就會漏
            }
        }
    }
    assert!(problems.is_empty(), "登錄與腳本不一致:{problems:#?}");
}

// B2:依 id 找得到
#[test]
fn B2_依id找得到基本模型() {
    for id in ["sensevoice", "silero-vad", "speaker"] {
        let s = spec(id).unwrap_or_else(|| panic!("spec({id}) 為 None"));
        assert_eq!(s.id, id);
    }
    let ids: Vec<&str> = BASE_PACKAGES.iter().map(|m: &ModelSpec| m.id).collect();
    assert_eq!(ids, vec!["sensevoice", "silero-vad", "speaker"]);
}

#[test]
fn B2_基本模型的目錄與檔名() {
    assert_eq!(SENSEVOICE.dir_name, SENSEVOICE_DIR_NAME);
    let names: Vec<&str> = SENSEVOICE.files.iter().map(|f| f.name).collect();
    assert_eq!(names, vec!["model.int8.onnx", "tokens.txt"]);
    assert_eq!(SILERO_VAD.dir_name, "silero-vad");
    assert_eq!(SILERO_VAD.files.iter().map(|f| f.name).collect::<Vec<_>>(), vec!["silero_vad.onnx"]);
    assert_eq!(SPEAKER.dir_name, "speaker");
    assert_eq!(SPEAKER.files.iter().map(|f| f.name).collect::<Vec<_>>(), vec![SPEAKER_FILE]);
}

// B3:腳本擺法與 app 內下載擺法都算可用
#[test]
fn B3_腳本或app擺法都算可用() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let models = data.join("models");
    let usable = |m: &ModelSpec| package_usable(m, data);

    assert!(!usable(&SENSEVOICE) && !usable(&SILERO_VAD) && !usable(&SPEAKER), "空目錄不該算可用");

    // SenseVoice 兩個檔缺一不可
    let sv = models.join(SENSEVOICE_DIR_NAME);
    touch(&sv.join("model.int8.onnx"));
    assert!(!usable(&SENSEVOICE), "只有 model 沒有 tokens");
    touch(&sv.join("tokens.txt"));
    assert!(usable(&SENSEVOICE));

    // Silero:腳本位置(根目錄)與 app 位置(子資料夾)
    touch(&models.join("silero_vad.onnx"));
    assert!(usable(&SILERO_VAD), "腳本位置 models/silero_vad.onnx");
    fs::remove_file(models.join("silero_vad.onnx")).unwrap();
    assert!(!usable(&SILERO_VAD));
    touch(&models.join("silero-vad").join("silero_vad.onnx"));
    assert!(usable(&SILERO_VAD), "app 位置 models/silero-vad/silero_vad.onnx");

    // 說話者模型同理
    touch(&models.join(SPEAKER_FILE));
    assert!(usable(&SPEAKER), "腳本位置");
    fs::remove_file(models.join(SPEAKER_FILE)).unwrap();
    assert!(!usable(&SPEAKER));
    touch(&models.join("speaker").join(SPEAKER_FILE));
    assert!(usable(&SPEAKER), "app 位置");
}

// B4:先找根目錄、再找子資料夾,都沒有回根目錄路徑
#[test]
fn B4_模型檔位置依序找根目錄與子資料夾() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    let root: PathBuf = models.join("silero_vad.onnx");
    let sub: PathBuf = models.join("silero-vad").join("silero_vad.onnx");

    assert_eq!(find_model_file(&models, "silero-vad", "silero_vad.onnx"), root, "都沒有回根目錄路徑");

    touch(&sub);
    assert_eq!(find_model_file(&models, "silero-vad", "silero_vad.onnx"), sub, "只有子資料夾");

    touch(&root);
    assert_eq!(find_model_file(&models, "silero-vad", "silero_vad.onnx"), root, "兩者都有以根目錄優先");
}
