//! 依語言路由正式辨識模型(參考 hayamimi):日文 → ReazonSpeech、英文 → Parakeet,其餘 → SenseVoice。

use crate::asr::{Lang, SenseVoice, SenseVoiceConfig, SherpaTransducer, TransducerConfig, Transcriber, Transcript};
use anyhow::Result;
use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;

/// 正式辨識用哪個模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    SenseVoice,
    ReazonSpeech,
    Parakeet,
}

/// 已載入的專用模型。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Available {
    pub reazonspeech: bool,
    pub parakeet: bool,
}

/// 鎖定語言優先,否則用偵測到的語言;該語言沒有已載入的專用模型就用 SenseVoice。
pub fn route(lid: Lang, lock: Option<Lang>, available: Available) -> Route {
    match lock.unwrap_or(lid) {
        Lang::Ja if available.reazonspeech => Route::ReazonSpeech,
        Lang::En if available.parakeet => Route::Parakeet,
        _ => Route::SenseVoice,
    }
}

const SHORT_KANJI_MAX: usize = 8;

fn is_han(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '々')
}

/// 全漢字短句(去標點 ≤ 8 字、無假名與諺文)常被字集判斷成中文;上一句是日文時沿用日文。
pub fn lid_with_prior(text: &str, detected: Lang, prev: Option<Lang>) -> Lang {
    if detected != Lang::Zh || prev != Some(Lang::Ja) {
        return detected;
    }
    let body: Vec<char> = text.chars().filter(|c| c.is_alphanumeric()).collect();
    if !body.is_empty() && body.len() <= SHORT_KANJI_MAX && body.iter().all(|&c| is_han(c)) {
        Lang::Ja
    } else {
        detected
    }
}

/// 要載入專用模型的語言:鎖定語言優先,其次允許語言,都沒有時 {En, Ja, Ko}。
pub fn resolve_load_set(lock: Option<Lang>, allowed: &HashSet<Lang>) -> HashSet<Lang> {
    match lock {
        Some(l) => HashSet::from([l]),
        None if !allowed.is_empty() => allowed.clone(),
        None => HashSet::from([Lang::En, Lang::Ja, Lang::Ko]),
    }
}

/// 載入選用的專用模型:檔案未就緒回 (None, None);建立失敗回 (None, 錯誤訊息),該語言退回 SenseVoice,
/// 不讓整個 session 啟動失敗。
pub fn load_optional<T>(label: &str, ready: bool, build: impl FnOnce() -> Result<T>) -> (Option<T>, Option<String>) {
    if !ready {
        return (None, None);
    }
    match build() {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(format!("{label}專用模型載入失敗,改用 SenseVoice:{e:#}"))),
    }
}

/// 模型目錄可用:在下載規格表裡要每個檔案大小正確,不在表裡(bench 的其他版本)只看目錄存在。
fn transducer_ready(dir: &Path) -> bool {
    match (dir.parent(), dir.file_name().and_then(|n| n.to_str())) {
        (Some(models_dir), Some(name)) => crate::model_download::dir_ready(models_dir, name),
        _ => dir.exists(),
    }
}

/// 依語言路由的正式辨識:鎖定語言直接路由;自動模式先用 SenseVoice 判語言(結果也當後備),
/// 再交給該語言的專用模型。專用模型回空字串時退回 SenseVoice 的結果。
pub struct RoutedTranscriber {
    sensevoice: SenseVoice,
    reazonspeech: Option<SherpaTransducer>,
    parakeet: Option<SherpaTransducer>,
    /// 上一句的語言(全漢字短句判斷用)。
    prev: Cell<Option<Lang>>,
    load_errors: Vec<String>,
}

impl RoutedTranscriber {
    /// 只載入 `load` 裡語言的專用模型;模型檔不齊就略過,建立失敗記進 `load_errors`(兩者都退回 SenseVoice)。
    /// `sv.language` 有值時 SenseVoice 鎖定該語言(鎖定韓文等沒有專用模型的語言時才會用到)。
    /// `pad_ms`:送專用模型前段首段尾墊的合成靜音(見 `TransducerConfig::pad_ms`)。
    pub fn new(sv: &SenseVoiceConfig, load: &HashSet<Lang>, reazonspeech_dir: &str, pad_ms: u32) -> Result<Self> {
        let ja = TransducerConfig { pad_ms, ..TransducerConfig::reazonspeech(reazonspeech_dir, sv.num_threads) };
        let en = TransducerConfig { pad_ms, ..TransducerConfig::parakeet(sv.num_threads) };
        let build = |label: &str, want: bool, cfg: TransducerConfig| {
            load_optional(label, want && transducer_ready(&cfg.dir), || SherpaTransducer::new(&cfg))
        };
        let (reazonspeech, ja_err) = build("日文", load.contains(&Lang::Ja), ja);
        let (parakeet, en_err) = build("英文", load.contains(&Lang::En), en);
        Ok(Self {
            sensevoice: SenseVoice::new(sv)?,
            reazonspeech,
            parakeet,
            prev: Cell::new(None),
            load_errors: ja_err.into_iter().chain(en_err).collect(),
        })
    }

    pub fn available(&self) -> Available {
        Available { reazonspeech: self.reazonspeech.is_some(), parakeet: self.parakeet.is_some() }
    }

    /// 建立失敗而退回 SenseVoice 的專用模型(給 UI 顯示)。
    pub fn load_errors(&self) -> &[String] {
        &self.load_errors
    }
}

impl Transcriber for RoutedTranscriber {
    fn transcribe(&self, pcm: &[f32], lang_hint: Option<Lang>) -> Result<Transcript> {
        let (lid, probe) = match lang_hint {
            Some(l) => (l, None),
            None => {
                let t = self.sensevoice.transcribe(pcm, None)?;
                (lid_with_prior(&t.text, t.lang, self.prev.get()), Some(t))
            }
        };
        let model = match route(lid, lang_hint, self.available()) {
            Route::ReazonSpeech => self.reazonspeech.as_ref(),
            Route::Parakeet => self.parakeet.as_ref(),
            Route::SenseVoice => None,
        };
        let out = match (model, probe) {
            (None, Some(p)) => Transcript { lang: lid, ..p },
            (None, None) => self.sensevoice.transcribe(pcm, lang_hint)?,
            (Some(m), probe) => {
                let t = m.transcribe(pcm, Some(lid))?;
                match probe {
                    Some(p) if t.text.is_empty() => Transcript { lang: lid, ..p },
                    Some(p) => Transcript { asr_ms: t.asr_ms + p.asr_ms, ..t },
                    None => t,
                }
            }
        };
        if out.lang != Lang::Unknown {
            self.prev.set(Some(out.lang));
        }
        Ok(out)
    }

    fn name(&self) -> &'static str {
        "routed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: Available = Available { reazonspeech: true, parakeet: true };

    fn set(items: &[Lang]) -> HashSet<Lang> {
        items.iter().copied().collect()
    }

    // T23:鎖定日文且 ReazonSpeech 已載入時走 ReazonSpeech
    #[test]
    fn route_lock_ja_uses_reazonspeech() {
        assert_eq!(route(Lang::En, Some(Lang::Ja), BOTH), Route::ReazonSpeech);
    }

    // T23:鎖定韓文沒有專用模型,走 SenseVoice
    #[test]
    fn route_lock_ko_uses_sensevoice() {
        assert_eq!(route(Lang::En, Some(Lang::Ko), BOTH), Route::SenseVoice);
    }

    // T23:鎖定日文但 ReazonSpeech 未載入時退回 SenseVoice
    #[test]
    fn route_lock_ja_without_reazonspeech_falls_back() {
        let av = Available { reazonspeech: false, parakeet: true };
        assert_eq!(route(Lang::Ja, Some(Lang::Ja), av), Route::SenseVoice);
    }

    // T23:未鎖定時依偵測語言:英文 Parakeet、日文 ReazonSpeech、中文 SenseVoice
    #[test]
    fn route_detected_language_without_lock() {
        assert_eq!(route(Lang::En, None, BOTH), Route::Parakeet);
        assert_eq!(route(Lang::Ja, None, BOTH), Route::ReazonSpeech);
        assert_eq!(route(Lang::Zh, None, BOTH), Route::SenseVoice);
    }

    // T23:偵測到英文但 Parakeet 未載入時用 SenseVoice
    #[test]
    fn route_en_without_parakeet_falls_back() {
        let av = Available { reazonspeech: true, parakeet: false };
        assert_eq!(route(Lang::En, None, av), Route::SenseVoice);
    }

    // T23:鎖定語言優先於偵測語言
    #[test]
    fn route_lock_overrides_detected() {
        assert_eq!(route(Lang::Ja, Some(Lang::En), BOTH), Route::Parakeet);
    }

    // T24:全漢字短句被判中文且上一句是日文時沿用日文
    #[test]
    fn lid_prior_ja_overrides_short_kanji_zh() {
        assert_eq!(lid_with_prior("大丈夫", Lang::Zh, Some(Lang::Ja)), Lang::Ja);
        assert_eq!(lid_with_prior("大丈夫。", Lang::Zh, Some(Lang::Ja)), Lang::Ja);
    }

    // T24:沒有上一句或上一句不是日文時維持偵測結果
    #[test]
    fn lid_prior_not_ja_keeps_detected() {
        assert_eq!(lid_with_prior("大丈夫", Lang::Zh, None), Lang::Zh);
        assert_eq!(lid_with_prior("大丈夫", Lang::Zh, Some(Lang::En)), Lang::Zh);
    }

    // T24:含假名的句子沿用偵測結果
    #[test]
    fn lid_prior_kana_text_keeps_detected() {
        assert_eq!(lid_with_prior("東京都知事選挙の結果発表", Lang::Ja, Some(Lang::En)), Lang::Ja);
        assert_eq!(lid_with_prior("東京都知事選挙の結果発表", Lang::Zh, Some(Lang::Ja)), Lang::Zh);
    }

    // T24:超過 8 字的全漢字句不沿用上一句語言
    #[test]
    fn lid_prior_long_kanji_keeps_detected() {
        let text = "東京都知事選挙結果発表";
        assert!(text.chars().count() > 8);
        assert_eq!(lid_with_prior(text, Lang::Zh, Some(Lang::Ja)), Lang::Zh);
    }

    // T25:鎖定語言優先於允許語言
    #[test]
    fn load_set_lock_wins() {
        assert_eq!(resolve_load_set(Some(Lang::Ja), &set(&[Lang::En, Lang::Ko])), set(&[Lang::Ja]));
    }

    // T25:未鎖定時用允許語言
    #[test]
    fn load_set_uses_allowed() {
        assert_eq!(resolve_load_set(None, &set(&[Lang::En])), set(&[Lang::En]));
    }

    // T25:都沒有時預設英日韓
    #[test]
    fn load_set_default_en_ja_ko() {
        assert_eq!(resolve_load_set(None, &HashSet::new()), set(&[Lang::En, Lang::Ja, Lang::Ko]));
    }

    // T20:專用模型建立失敗時回 (None, 含標籤與原因的訊息)
    #[test]
    fn load_optional_build_failure_reports_error() {
        let (v, err) = load_optional("日文", true, || -> anyhow::Result<u8> { Err(anyhow::anyhow!("壞檔")) });
        assert_eq!(v, None);
        let msg = err.expect("建立失敗應回錯誤訊息");
        assert!(msg.contains("日文"), "實得 {msg:?}");
        assert!(msg.contains("壞檔"), "實得 {msg:?}");
    }

    // T20:檔案未就緒時回 (None, None) 且不呼叫 build
    #[test]
    fn load_optional_not_ready_skips_build() {
        let calls = std::cell::Cell::new(0u32);
        let (v, err) = load_optional("日文", false, || -> anyhow::Result<u8> {
            calls.set(calls.get() + 1);
            Ok(1)
        });
        assert_eq!(v, None);
        assert_eq!(err, None);
        assert_eq!(calls.get(), 0, "未就緒不應呼叫 build");
    }

    // T20:建立成功時回 (Some(值), None)
    #[test]
    fn load_optional_success_returns_value() {
        let (v, err) = load_optional("日文", true, || -> anyhow::Result<u8> { Ok(7) });
        assert_eq!(v, Some(7));
        assert_eq!(err, None);
    }
}
