//! 新辨識引擎(Qwen3-ASR、Cohere Transcribe)與並排比較表:輸出解析、缺檔錯誤、表格格式。
#![allow(non_snake_case)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use tingyi_core::asr::{
    parse_qwen3_output, CohereConfig, Lang, Qwen3Config, SherpaCohere, SherpaQwen3,
};
use tingyi_core::eval::{render_compare_table, CompareRow};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 在系統暫存目錄下建一個唯一的空目錄。
fn unique_empty_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("tingyi_new_asr_{tag}_{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// Q1:帶 `language X<asr_text>` 前綴時,剝掉前綴並依前綴語言回傳
#[test]
fn Q1_qwen3_strips_language_prefix_ja() {
    let (text, lang) = parse_qwen3_output("language Japanese<asr_text>こんにちは");
    assert_eq!(text, "こんにちは");
    assert_eq!(lang, Lang::Ja);
}

// Q1:英文前綴 → 文字不含前綴、語言為 En
#[test]
fn Q1_qwen3_strips_language_prefix_en() {
    let (text, lang) = parse_qwen3_output("language English<asr_text>hello");
    assert_eq!(text, "hello");
    assert_eq!(lang, Lang::En);
}

// Q1:韓文前綴 → 文字不含前綴、語言為 Ko
#[test]
fn Q1_qwen3_strips_language_prefix_ko() {
    let (text, lang) = parse_qwen3_output("language Korean<asr_text>안녕하세요");
    assert_eq!(text, "안녕하세요");
    assert_eq!(lang, Lang::Ko);
}

// Q2:沒有前綴時文字原樣保留,語言交給 detect_lang(英文 → En)
#[test]
fn Q2_qwen3_without_prefix_keeps_text() {
    let (text, lang) = parse_qwen3_output("hello world");
    assert_eq!(text, "hello world");
    assert_eq!(lang, Lang::En);
}

// Q2:沒有前綴且前後有空白時要 trim
#[test]
fn Q2_qwen3_without_prefix_trims_whitespace() {
    let (text, _lang) = parse_qwen3_output("  \n hello world \t ");
    assert_eq!(text, "hello world");
}

// Q2:有前綴時前綴後的前後空白也要 trim
#[test]
fn Q2_qwen3_with_prefix_trims_whitespace() {
    let (text, lang) = parse_qwen3_output("language Japanese<asr_text>  こんにちは \n");
    assert_eq!(text, "こんにちは");
    assert_eq!(lang, Lang::Ja);
}

// Q3:模型目錄是空的時,Qwen3 建立失敗且錯誤訊息點名缺 encoder
#[test]
fn Q3_qwen3_empty_dir_errors_mentioning_encoder() {
    let dir = unique_empty_dir("qwen3");
    let cfg = Qwen3Config { dir: dir.clone(), num_threads: 1, name: "qwen3-test" };
    let err = SherpaQwen3::new(&cfg).err().expect("空目錄不該建立成功");
    let msg = format!("{err:#}");
    assert!(msg.contains("encoder"), "錯誤訊息應提到 encoder,實際:{msg}");
    let _ = fs::remove_dir_all(&dir);
}

// Q3:模型目錄是空的時,Cohere 建立失敗且錯誤訊息點名缺 encoder
#[test]
fn Q3_cohere_empty_dir_errors_mentioning_encoder() {
    let dir = unique_empty_dir("cohere");
    let cfg = CohereConfig { dir: dir.clone(), num_threads: 1, lang: Lang::En, name: "cohere-test" };
    let err = SherpaCohere::new(&cfg).err().expect("空目錄不該建立成功");
    let msg = format!("{err:#}");
    assert!(msg.contains("encoder"), "錯誤訊息應提到 encoder,實際:{msg}");
    let _ = fs::remove_dir_all(&dir);
}

fn is_separator(line: &str) -> bool {
    line.contains("---") && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// 去掉被跳脫的 `\|` 後,數剩下的欄位分隔 `|`。
fn unescaped_pipes(line: &str) -> usize {
    line.replace("\\|", "").matches('|').count()
}

// Q4:2 個引擎 2 列資料 → 表頭含引擎名、有分隔列、恰好 2 列資料,且時間/耗時/跳脫都正確
#[test]
fn Q4_compare_table_layout_time_elapsed_and_escape() {
    let rows = vec![
        CompareRow {
            start_ms: 1500,
            end_ms: 3000,
            cells: vec![("x|y".to_string(), 1234), ("second".to_string(), 567)],
        },
        CompareRow {
            start_ms: 4000,
            end_ms: 6500,
            cells: vec![("plain".to_string(), 89), ("也可以".to_string(), 4321)],
        },
    ];
    let out = render_compare_table(&["a", "b"], &rows);
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();

    // 結構:表頭 + 分隔 + 2 列資料 = 4 行
    assert_eq!(lines.len(), 4, "應為表頭+分隔+2 列資料,實際:\n{out}");
    let header = lines[0];
    assert!(header.contains('a') && header.contains('b'), "表頭要含兩個引擎名:{header}");
    assert!(is_separator(lines[1]), "第二行應是分隔列:{}", lines[1]);
    assert!(!is_separator(lines[2]) && !is_separator(lines[3]));

    // 每個資料列的欄位數(未跳脫的 |)要和表頭一致,文字裡的 | 不能撐出多餘欄位
    let header_pipes = unescaped_pipes(header);
    assert_eq!(unescaped_pipes(lines[1]), header_pipes, "分隔列欄數要和表頭一致");
    assert_eq!(unescaped_pipes(lines[2]), header_pipes, "第 1 列欄數要和表頭一致:{}", lines[2]);
    assert_eq!(unescaped_pipes(lines[3]), header_pipes, "第 2 列欄數要和表頭一致:{}", lines[3]);

    // 跳脫
    assert!(lines[2].contains("x\\|y"), "文字中的 | 要跳脫成 \\|:{}", lines[2]);

    // 時間以秒顯示
    assert!(lines[2].contains("1.5"), "第 1 列應顯示起點 1.5 秒:{}", lines[2]);
    assert!(lines[3].contains("4.0") || lines[3].contains("4 ") || lines[3].contains("4.00"), "第 2 列應顯示起點 4 秒:{}", lines[3]);

    // 每格顯示耗時 ms
    assert!(lines[2].contains("1234") && lines[2].contains("567"), "第 1 列兩格耗時:{}", lines[2]);
    assert!(lines[3].contains("89") && lines[3].contains("4321"), "第 2 列兩格耗時:{}", lines[3]);

    // 文字內容也在
    assert!(lines[2].contains("second") && lines[3].contains("plain") && lines[3].contains("也可以"));
}

// Q5:模型沒停在結束符號、後面接著亂生時,只保留結束符號前的文字
#[test]
fn Q5_qwen3_truncates_at_end_of_text_token() {
    let (text, lang) = tingyi_core::asr::parse_qwen3_output("住宅街っぽい感じがします。<|endoftext|>Human Rights Watchは、調査しました。");
    assert_eq!(text, "住宅街っぽい感じがします。");
    assert_eq!(lang, tingyi_core::asr::Lang::Ja);
    let (text, _) = tingyi_core::asr::parse_qwen3_output("language English<asr_text>hello<|im_end|>garbage");
    assert_eq!(text, "hello");
}
