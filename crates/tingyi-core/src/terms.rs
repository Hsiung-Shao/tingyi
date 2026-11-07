//! 遊戲術語包(例如流放之路):由 `scripts/poe-terms/build_term_pack.py` 從本機 POE DB 產生的 JSON。
//! 三種用途:原文的同音誤聽修正、Whisper 熱詞提示、每句命中的術語交給翻譯模型。

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::postprocess::GlossaryEntry;

/// Whisper 熱詞提示的總長上限(字元)。whisper prompt 約 224 token,英文專名約 4 字元一 token。
pub const HOTWORD_MAX_CHARS: usize = 400;
/// 每句最多交給翻譯模型的術語數。
pub const MAX_HITS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TermEntry {
    pub en: String,
    pub zh: String,
    /// 主要出處 `tbl.col`
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub count: u32,
    /// 其他官方譯名
    #[serde(default)]
    pub alt: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Alias {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TermPack {
    pub game: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub game_version: String,
    #[serde(default)]
    pub db_version: u32,
    #[serde(default)]
    pub generated_at: String,
    pub entries: Vec<TermEntry>,
    #[serde(default)]
    pub aliases: Vec<Alias>,
    #[serde(default)]
    pub hotwords: Vec<String>,
    /// 比對用索引(第一次比對時建立;建構時用 `..Default::default()` 即可)
    #[serde(skip)]
    pub index: TermIndex,
}

/// 小寫、以單一空白相連的原文 → entries 索引,與最長詞的字數。
#[derive(Debug, Clone, Default)]
pub struct TermIndex(std::sync::OnceLock<(HashMap<String, usize>, usize)>);

impl PartialEq for TermPack {
    fn eq(&self, o: &Self) -> bool {
        self.game == o.game && self.entries == o.entries && self.aliases == o.aliases && self.hotwords == o.hotwords
    }
}

/// 字界判斷用:ASCII 英數(字中的 `'`、`’`、`-` 由 `tokenize` 另外處理)。
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// 把文字切成字,回 (起始 byte, 結束 byte, 小寫字)。
fn tokenize(text: &str) -> Vec<(usize, usize, String)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !is_word_byte(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        loop {
            while i < b.len() && is_word_byte(b[i]) {
                i += 1;
            }
            // 字中的 ' ’ -:後面還接英數才算同一個字
            let joiner = if text[i..].starts_with('\u{2019}') {
                3
            } else if i < b.len() && (b[i] == b'\'' || b[i] == b'-') {
                1
            } else {
                0
            };
            if joiner > 0 && i + joiner < b.len() && is_word_byte(b[i + joiner]) {
                i += joiner;
                continue;
            }
            break;
        }
        out.push((start, i, text[start..i].to_ascii_lowercase().replace('\u{2019}', "'")));
    }
    out
}

/// 一個字可能的單數寫法(含原字):Kitava's、orbs、boxes、bodies、staves/knives。
fn singular_forms(w: &str) -> Vec<String> {
    let mut v = vec![w.to_string()];
    if let Some(s) = w.strip_suffix("'s") {
        v.push(s.to_string());
    }
    if let Some(s) = w.strip_suffix("ies") {
        v.push(format!("{s}y"));
    }
    if let Some(s) = w.strip_suffix("ves") {
        // wolves→wolf、knives→knife、staves→staff
        v.push(format!("{s}f"));
        v.push(format!("{s}fe"));
        v.push(format!("{s}ff"));
    }
    if let Some(s) = w.strip_suffix("es") {
        v.push(s.to_string());
    }
    if let Some(s) = w.strip_suffix('s') {
        v.push(s.to_string());
    }
    v
}

impl TermPack {
    /// 讀術語包;壞檔回錯且訊息含檔名。
    pub fn load(path: &Path) -> anyhow::Result<TermPack> {
        let text = std::fs::read_to_string(path).with_context(|| format!("讀不到術語包 {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("術語包格式錯誤 {}", path.display()))
    }

    fn index(&self) -> &(HashMap<String, usize>, usize) {
        self.index.0.get_or_init(|| {
            let mut map = HashMap::new();
            let mut max_words = 1;
            for (i, e) in self.entries.iter().enumerate() {
                let key = tokenize(&e.en).into_iter().map(|t| t.2).collect::<Vec<_>>().join(" ");
                if key.is_empty() {
                    continue;
                }
                max_words = max_words.max(key.split(' ').count());
                map.entry(key).or_insert(i);
            }
            (map, max_words)
        })
    }

    /// 本句命中的術語(不分大小寫、整字、含常見複數;長詞優先不重疊;依出現順序;最多 `max` 個)。
    pub fn hits(&self, text: &str, max: usize) -> Vec<&TermEntry> {
        let (map, max_words) = self.index();
        let toks = tokenize(text);
        let mut out: Vec<&TermEntry> = Vec::new();
        let mut i = 0;
        while i < toks.len() && out.len() < max {
            let mut matched = 0;
            for n in (1..=(*max_words).min(toks.len() - i)).rev() {
                let words: Vec<&str> = toks[i..i + n].iter().map(|t| t.2.as_str()).collect();
                if let Some(&idx) = lookup(map, &words) {
                    let e = &self.entries[idx];
                    if !out.iter().any(|x| std::ptr::eq(*x, e)) {
                        out.push(e);
                    }
                    matched = n;
                    break;
                }
            }
            i += matched.max(1);
        }
        out
    }

    /// 同音誤聽修正:不分大小寫、整字取代成正確拼寫;長的先比、取代後不再連鎖。
    pub fn correct_aliases(&self, text: &str) -> String {
        let mut rules: Vec<(&str, &str)> = self
            .aliases
            .iter()
            .filter(|a| !a.from.trim().is_empty() && a.from.is_ascii())
            .map(|a| (a.from.trim(), a.to.as_str()))
            .collect();
        if rules.is_empty() {
            return text.to_string();
        }
        rules.sort_by_key(|r| std::cmp::Reverse(r.0.len()));
        let b = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        'scan: while i < text.len() {
            let at_start = i == 0 || !is_word_byte(b[i - 1]);
            if at_start {
                for (from, to) in &rules {
                    let end = i + from.len();
                    if end <= b.len()
                        && b[i..end].eq_ignore_ascii_case(from.as_bytes())
                        && (end == b.len() || !is_word_byte(b[end]))
                    {
                        out.push_str(to);
                        i = end;
                        continue 'scan;
                    }
                }
            }
            let c = text[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
        out
    }

    /// 查詢:英文不分大小寫、中文子字串;完全相符(英文或中文)排前面,其餘依英文長度;最多 `limit` 筆。
    pub fn search(&self, q: &str, limit: usize) -> Vec<&TermEntry> {
        let q = q.trim();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        let ql = q.to_lowercase();
        let mut found: Vec<&TermEntry> = self
            .entries
            .iter()
            .filter(|e| e.en.to_lowercase().contains(&ql) || e.zh.contains(q))
            .collect();
        // 完全相符在前,其餘依英文長度(短的通常是基本詞)
        found.sort_by_key(|e| (!(e.en.eq_ignore_ascii_case(q) || e.zh == q), e.en.len()));
        found.truncate(limit);
        found
    }

    /// Whisper 熱詞提示:只用正確拼寫(`hotwords`),以「, 」連接,總長不超過 `HOTWORD_MAX_CHARS`。
    pub fn hotword_prompt(&self) -> String {
        let mut out = String::new();
        for w in self.hotwords.iter().map(|w| w.trim()).filter(|w| !w.is_empty()) {
            let add = if out.is_empty() { w.chars().count() } else { w.chars().count() + 2 };
            if out.chars().count() + add > HOTWORD_MAX_CHARS {
                break;
            }
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(w);
        }
        out
    }
}

/// 以多種單複數寫法查字典:最後一個字去複數;`X of Y` 的第一個字去複數(Orbs of Alteration)。
fn lookup<'a>(map: &'a HashMap<String, usize>, words: &[&str]) -> Option<&'a usize> {
    let n = words.len();
    let last_forms = singular_forms(words[n - 1]);
    let first_forms = if n >= 3 && words[1] == "of" { singular_forms(words[0]) } else { vec![words[0].to_string()] };
    for f in &first_forms {
        for l in &last_forms {
            let key = if n == 1 {
                l.clone()
            } else {
                let mut parts: Vec<&str> = words.to_vec();
                parts[0] = f;
                parts[n - 1] = l;
                parts.join(" ")
            };
            if let Some(i) = map.get(&key) {
                return Some(i);
            }
        }
    }
    None
}

/// 術語包摘要(字幕頁「遊戲術語」選單用)。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackInfo {
    /// 檔名(不含 .json),開始時以它指定術語包
    pub id: String,
    pub label: String,
    pub game_version: String,
    pub entries: usize,
    pub aliases: usize,
    pub hotwords: usize,
    pub generated_at: String,
}

impl PackInfo {
    pub fn of(id: &str, t: &TermPack) -> PackInfo {
        PackInfo {
            id: id.to_string(),
            label: if t.label.is_empty() { t.game.clone() } else { t.label.clone() },
            game_version: t.game_version.clone(),
            entries: t.entries.len(),
            aliases: t.aliases.len(),
            hotwords: t.hotwords.len(),
            generated_at: t.generated_at.clone(),
        }
    }
}

/// 匯入術語包:驗證(能解析、game 與 entries 非空、game 是合法 id)後複製到 `dir/<game>.json`;
/// 先寫暫存檔再改名,失敗不留檔。已存在會覆蓋(要不要覆蓋由呼叫端先用 `pack_exists` 問)。
pub fn import_pack(src: &Path, dir: &Path) -> anyhow::Result<PackInfo> {
    let pack = TermPack::load(src)?;
    if pack.game.trim().is_empty() {
        anyhow::bail!("不是術語包:缺少 game 欄位({})", src.display());
    }
    if !valid_pack_id(&pack.game) {
        anyhow::bail!("術語包的 game 只能是英數、底線、連字號:{:?}", pack.game);
    }
    if pack.entries.is_empty() {
        anyhow::bail!("術語包沒有任何術語({})", src.display());
    }
    let bytes = std::fs::read(src).with_context(|| format!("讀不到 {}", src.display()))?;
    let dest = dir.join(format!("{}.json", pack.game));
    let tmp = dir.join(format!(".{}.json.{}.tmp", pack.game, std::process::id()));
    let write = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &dest));
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("寫入 {} 失敗", dest.display()));
    }
    Ok(PackInfo::of(&pack.game, &pack))
}

/// `dir/<id>.json` 是否存在(id 不合法回 false)。
pub fn pack_exists(dir: &Path, id: &str) -> bool {
    valid_pack_id(id) && dir.join(format!("{id}.json")).is_file()
}

/// 移除 `dir/<id>.json`;id 不合法回錯。
pub fn remove_pack(dir: &Path, id: &str) -> anyhow::Result<()> {
    if !valid_pack_id(id) {
        anyhow::bail!("術語包名稱不合法:{id}");
    }
    let path = dir.join(format!("{id}.json"));
    std::fs::remove_file(&path).with_context(|| format!("刪除 {} 失敗", path.display()))
}

/// 列出目錄裡的術語包(依 id 排序);讀不了的檔略過並記 log。
pub fn list_packs(dir: &Path) -> Vec<PackInfo> {
    let mut out: Vec<PackInfo> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        // 與開始時的檢查同一條規則:檔名不合法(例如 poe1.bak.json)不列出
        .filter(|p| p.file_stem().and_then(|s| s.to_str()).is_some_and(valid_pack_id))
        .filter_map(|p| match TermPack::load(&p) {
            Ok(t) => Some(PackInfo::of(&p.file_stem()?.to_string_lossy(), &t)),
            Err(e) => {
                tracing::warn!("{e:#}");
                None
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 交給翻譯模型的術語:使用者術語表命中在前;術語包命中若原文(不分大小寫)已在使用者表裡就略過。
pub fn merge_glossary(user: Vec<GlossaryEntry>, pack_hits: &[&TermEntry]) -> Vec<GlossaryEntry> {
    let mut out = user;
    for e in pack_hits {
        if !out.iter().any(|g| g.source.eq_ignore_ascii_case(&e.en)) {
            out.push(GlossaryEntry { source: e.en.clone(), target: e.zh.clone() });
        }
    }
    out
}

/// 術語包 id(檔名不含 .json)是否合法:只收 ASCII 英數、底線、連字號。
pub fn valid_pack_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 每句交給翻譯模型的術語(快路徑與回修共用)。
/// 術語包的譯名是繁體中文,只有目標是繁中時才併入(譯成英日韓或簡體時放進去反而混進繁中字)。
pub fn translation_glossary(user: &crate::postprocess::Glossary, pack: Option<&TermPack>, text: &str, target: crate::mt::Target) -> Vec<GlossaryEntry> {
    let user_hits: Vec<GlossaryEntry> = user.hits(text).into_iter().cloned().collect();
    match pack {
        Some(p) if target == crate::mt::Target::ZhTw => merge_glossary(user_hits, &p.hits(text, MAX_HITS)),
        _ => user_hits,
    }
}
