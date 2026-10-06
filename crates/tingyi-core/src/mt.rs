//! 翻譯:統一介面 `Translator` + `LlamaServerTranslator`(OpenAI 相容串流端點)。
//!
//! 本機 llama-server 與雲端 OpenAI 相容服務共用同一套程式碼。
//! 提示範本依模型家族切換(`PromptStyle`):
//! - Hy-MT2:依模型卡(簡體原文),目標語言「繁體中文」,支援背景前文與術語。
//! - Sakura:依 SakuraLLM v1.0 README 的 system / user 格式(日→簡中,後處理再轉繁體),
//!   不送背景前文(模型未以此格式訓練),術語表用 `src->dst` 每行一筆。
//! - Seed-X:無 chat 範本,走 `/completion` 原始提示,結尾必須帶 `<zh>` 語言標籤(PPO 訓練格式)。
//! - LMT-60:chat user 訊息「Translate the following text from X into Chinese:」。
//! - TranslateGemma:官方範本需要結構化 content,llama-server 吃不到語言參數,
//!   所以自己渲染成 Gemma 回合格式走 `/completion`。
//! 除 Hy-MT2 外都不送背景前文、也不送術語(各模型沒有對應格式)。

use anyhow::{anyhow, Context, Result};
use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::asr::Lang;
use crate::postprocess::GlossaryEntry;

/// 一次翻譯請求。
#[derive(Debug, Clone, Default)]
pub struct TranslateRequest {
    pub text: String,
    pub src_lang: Lang,
    /// 最近幾句的(原文, 譯文),舊 → 新。
    pub context: Vec<(String, String)>,
    pub glossary: Vec<GlossaryEntry>,
    /// 這句之後的幾句原文(回修用,只供理解語境)。快路徑永遠空 → 提示位元組不變。
    pub lookahead: Vec<String>,
    /// llama-server slot(`id_slot`);None 由伺服器挑。回修用 1、快路徑用 0,互不洗掉快取。
    pub slot: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct TranslateResult {
    pub text: String,
    pub first_token_ms: Option<u32>,
    pub total_ms: u32,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    /// llama-server 回傳的 timings(若有)。
    pub server_prompt_ms: Option<f32>,
    pub server_predicted_ms: Option<f32>,
}

/// 提示範本家族,依模型檔名決定(含 "sakura" → Sakura,其餘 Hy-MT2)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PromptStyle {
    #[default]
    HyMt2,
    Sakura,
    SeedX,
    Lmt,
    TranslateGemma,
}

impl PromptStyle {
    pub fn for_model(model_name: &str) -> Self {
        let n = model_name.to_ascii_lowercase();
        if n.contains("sakura") {
            Self::Sakura
        } else if n.contains("seed-x") {
            Self::SeedX
        } else if n.contains("lmt-60") {
            Self::Lmt
        } else if n.contains("translategemma") {
            Self::TranslateGemma
        } else {
            Self::HyMt2
        }
    }

    /// 是否走 llama-server 原始 `/completion`(自己渲染回合格式)而非 chat 端點。
    pub fn raw_completion(self) -> bool {
        matches!(self, Self::SeedX | Self::TranslateGemma)
    }
}

/// 英文語言名(給英文提示的模型)。Unknown 以 English 代替,實際由模型自行判斷。
fn lang_name(l: Lang) -> &'static str {
    match l {
        Lang::En | Lang::Unknown => "English",
        Lang::Ja => "Japanese",
        Lang::Ko => "Korean",
        Lang::Zh => "Chinese",
    }
}

/// Seed-X 原始提示(模型卡格式;`<zh>` 標籤必要)。
pub fn build_seedx_prompt(req: &TranslateRequest) -> String {
    format!(
        "Translate the following {} sentence into Chinese:\n{} <zh>",
        lang_name(req.src_lang),
        req.text
    )
}

/// LMT-60 user 訊息(README 範例格式)。
pub fn build_lmt_prompt(req: &TranslateRequest) -> String {
    let src = lang_name(req.src_lang);
    format!(
        "Translate the following text from {src} into Chinese:\n{src}: {}\nChinese:",
        req.text
    )
}

/// TranslateGemma 官方範本渲染後的文字(取自 llama.cpp issue #19295 的輸出),包成 Gemma 回合。
pub fn build_translategemma_prompt(req: &TranslateRequest) -> String {
    let src = lang_name(req.src_lang);
    let code = req.src_lang.code();
    let body = format!(
        "You are a professional {src} ({code}) to Chinese (zh-TW) translator. Your goal is to accurately convey the meaning and nuances of the original {src} text while adhering to Chinese grammar, vocabulary, and cultural sensitivities. Produce only the Chinese translation, without any additional explanations or commentary. Please translate the following {src} text into Chinese:\n\n\n{}",
        req.text
    );
    format!("<start_of_turn>user\n{body}<end_of_turn>\n<start_of_turn>model\n")
}

/// 原始 `/completion` 用的提示字串(只對 `raw_completion()` 的範本有意義)。
pub fn build_raw_prompt(req: &TranslateRequest, style: PromptStyle) -> String {
    match style {
        PromptStyle::SeedX => build_seedx_prompt(req),
        PromptStyle::TranslateGemma => build_translategemma_prompt(req),
        // 其他範本不走原始端點;給一個可用的退路而非 panic
        _ => build_prompt(req),
    }
}

/// SakuraLLM v1.0 的 system prompt(README 原文,簡體)。
pub const SAKURA_SYSTEM: &str = "你是一个轻小说翻译模型，可以流畅通顺地以日本轻小说的风格将日文翻译成简体中文，并联系上下文正确使用人称代词，不擅自添加原文中没有的代词。";

/// 組出送給 chat 端點的訊息(role, content)。
pub fn build_messages(req: &TranslateRequest, style: PromptStyle) -> Vec<(&'static str, String)> {
    match style {
        PromptStyle::HyMt2 => vec![("user", build_prompt(req))],
        PromptStyle::Sakura => vec![
            ("system", SAKURA_SYSTEM.to_string()),
            ("user", build_sakura_prompt(req)),
        ],
        PromptStyle::Lmt => vec![("user", build_lmt_prompt(req))],
        PromptStyle::SeedX | PromptStyle::TranslateGemma => vec![("user", build_raw_prompt(req, style))],
    }
}

/// Sakura v1.0 user prompt:有術語表與無術語表兩種(README 建議無術語時用短版)。
pub fn build_sakura_prompt(req: &TranslateRequest) -> String {
    if req.glossary.is_empty() {
        format!("将下面的日文文本翻译成中文：{}", req.text)
    } else {
        let dict: Vec<String> = req
            .glossary
            .iter()
            .map(|g| format!("{}->{}", g.source, g.target))
            .collect();
        format!(
            "根据以下术语表（可以为空）：\n{}\n将下面的日文文本根据对应关系和备注翻译成中文：{}",
            dict.join("\n"),
            req.text
        )
    }
}

/// 串流回呼:每收到一段增量文字就呼叫。
pub type OnDelta<'a> = &'a mut (dyn FnMut(&str) + Send);

pub trait Translator: Send + Sync {
    fn translate<'a>(
        &'a self,
        req: TranslateRequest,
        on_delta: OnDelta<'a>,
    ) -> BoxFuture<'a, Result<TranslateResult>>;
    fn name(&self) -> String;
}

/// 組出 Hy-MT2 提示(純函式,可測)。
///
/// 規則:
/// - 有術語命中 → 前置「参考下面的翻译:」清單。
/// - 有上下文 → 以【背景信息】附最近幾句原文與譯文。
/// - 風格要求併入最後一行指令(口語、簡潔、台灣用語)。
pub fn build_prompt(req: &TranslateRequest) -> String {
    const TARGET: &str = "繁体中文(台湾用语)";
    let mut p = String::new();
    let has_bg = !req.context.is_empty() || !req.lookahead.is_empty();
    if has_bg {
        p.push_str("【背景信息】\n");
    }
    if !req.context.is_empty() {
        p.push_str("以下是同一段影片中前面几句的原文与译文,请保持称谓、语气与用词一致:\n");
        for (s, t) in &req.context {
            p.push_str(s);
            p.push_str(" => ");
            p.push_str(t);
            p.push('\n');
        }
        p.push('\n');
    }
    if !req.lookahead.is_empty() {
        p.push_str("以下是这句之后的几句原文(尚未翻译),仅供理解语境,不要翻译它们:\n");
        for s in &req.lookahead {
            p.push_str(s);
            p.push('\n');
        }
        p.push('\n');
    }
    if !req.glossary.is_empty() {
        p.push_str("参考下面的翻译:\n");
        for g in &req.glossary {
            p.push_str(&format!("{} 翻译成 {}\n", g.source, g.target));
        }
        p.push('\n');
    }
    if !has_bg {
        p.push_str(&format!(
            "将以下文本翻译为{TARGET},这是影片字幕,请用台湾口语、简洁自然,注意只需要输出翻译后的结果,不要额外解释:\n\n{}",
            req.text
        ));
    } else {
        p.push_str(&format!(
            "请结合背景信息将以下文本翻译为{TARGET},这是影片字幕,请用台湾口语、简洁自然,注意只需要输出翻译后的结果,不要额外解释。\n\n【待翻译文本】\n{}",
            req.text
        ));
    }
    p
}

#[derive(Debug, Clone)]
pub struct LlamaServerConfig {
    /// 例如 `http://127.0.0.1:8178`
    pub base_url: String,
    /// 任意字串;llama-server 忽略,雲端服務需正確模型名。
    pub model: String,
    pub api_key: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    /// 整句逾時;超過先顯示原文。
    pub timeout: Duration,
    /// 提示範本家族。
    pub style: PromptStyle,
    /// 核取樣;None 用伺服器預設。Sakura README 建議 0.3。
    pub top_p: Option<f32>,
}

impl Default for LlamaServerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8178".into(),
            model: "hy-mt2".into(),
            api_key: None,
            max_tokens: 200,
            temperature: 0.0,
            timeout: Duration::from_secs(15),
            style: PromptStyle::HyMt2,
            top_p: None,
        }
    }
}

impl LlamaServerConfig {
    /// 依模型檔名挑範本與取樣參數(Sakura README:temperature 0.1、top_p 0.3)。
    pub fn for_model(base_url: impl Into<String>, model_name: &str) -> Self {
        let style = PromptStyle::for_model(model_name);
        let (temperature, top_p) = match style {
            PromptStyle::Sakura => (0.1, Some(0.3)),
            // Seed-X / LMT-60 / TranslateGemma 模型卡皆建議 greedy
            _ => (0.0, None),
        };
        Self {
            base_url: base_url.into(),
            model: model_name.to_string(),
            temperature,
            top_p,
            style,
            ..Default::default()
        }
    }
}

pub struct LlamaServerTranslator {
    cfg: LlamaServerConfig,
    client: reqwest::Client,
}

impl LlamaServerTranslator {
    pub fn new(cfg: LlamaServerConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .expect("reqwest client");
        Self { cfg, client }
    }

    pub async fn health(&self) -> Result<bool> {
        let url = format!("{}/health", self.cfg.base_url.trim_end_matches('/'));
        let r = self.client.get(&url).send().await.context("GET /health")?;
        Ok(r.status().is_success())
    }

    /// 等待伺服器就緒(模型載入完成)。
    pub async fn wait_ready(&self, max_wait: Duration) -> Result<()> {
        let t0 = Instant::now();
        loop {
            if let Ok(true) = self.health().await {
                return Ok(());
            }
            if t0.elapsed() > max_wait {
                return Err(anyhow!("llama-server {} 在 {:?} 內未就緒", self.cfg.base_url, max_wait));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    id_slot: Option<u32>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    max_tokens: u32,
    cache_prompt: bool,
    stream_options: StreamOptions,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// llama-server 原生 `/completion` 請求。
#[derive(Serialize)]
struct CompletionRequest<'a> {
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    id_slot: Option<u32>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    n_predict: u32,
    cache_prompt: bool,
}

/// `/completion` 串流每筆事件。
#[derive(Deserialize, Default)]
struct CompletionChunk {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tokens_predicted: Option<u32>,
    #[serde(default)]
    tokens_evaluated: Option<u32>,
    #[serde(default)]
    timings: Option<Timings>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Default)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    timings: Option<Timings>,
}

#[derive(Deserialize, Default)]
struct ChunkChoice {
    #[serde(default)]
    delta: Delta,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize, Default)]
struct Usage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
}

#[derive(Deserialize, Default)]
struct Timings {
    prompt_ms: Option<f32>,
    predicted_ms: Option<f32>,
}

impl Translator for LlamaServerTranslator {
    fn translate<'a>(
        &'a self,
        req: TranslateRequest,
        on_delta: OnDelta<'a>,
    ) -> BoxFuture<'a, Result<TranslateResult>> {
        Box::pin(async move {
            let t0 = Instant::now();
            let raw = self.cfg.style.raw_completion();
            let base = self.cfg.base_url.trim_end_matches('/');
            let msgs = build_messages(&req, self.cfg.style);
            let raw_prompt = build_raw_prompt(&req, self.cfg.style);
            // 雲端 OpenAI 相容端點不認 id_slot(只有本機 llama-server 用)
            let id_slot = if self.cfg.api_key.is_some() { None } else { req.slot };
            let mut rb = if raw {
                self.client.post(format!("{base}/completion")).json(&CompletionRequest {
                    prompt: &raw_prompt,
                    stream: true,
                    id_slot,
                    temperature: self.cfg.temperature,
                    top_p: self.cfg.top_p,
                    n_predict: self.cfg.max_tokens,
                    cache_prompt: true,
                })
            } else {
                self.client.post(format!("{base}/v1/chat/completions")).json(&ChatRequest {
                    model: &self.cfg.model,
                    messages: msgs
                        .iter()
                        .map(|(role, content)| ChatMessage { role, content })
                        .collect(),
                    stream: true,
                    id_slot,
                    temperature: self.cfg.temperature,
                    top_p: self.cfg.top_p,
                    max_tokens: self.cfg.max_tokens,
                    cache_prompt: true,
                    stream_options: StreamOptions { include_usage: true },
                })
            };
            if let Some(k) = &self.cfg.api_key {
                rb = rb.bearer_auth(k);
            }
            let resp = rb.send().await.context("POST 翻譯端點")?;
            if !resp.status().is_success() {
                let st = resp.status();
                let txt = resp.text().await.unwrap_or_default();
                return Err(anyhow!("翻譯服務回 {st}: {txt}"));
            }

            let mut out = TranslateResult::default();
            let mut text = String::new();
            let mut buf = String::new();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("讀取串流")?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
                // SSE:以空行分隔事件,每行 `data: {...}`
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..=pos);
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    if raw {
                        let Ok(c) = serde_json::from_str::<CompletionChunk>(data) else { continue };
                        if !c.content.is_empty() {
                            if out.first_token_ms.is_none() {
                                out.first_token_ms = Some(t0.elapsed().as_millis() as u32);
                            }
                            text.push_str(&c.content);
                            on_delta(&c.content);
                        }
                        if c.tokens_predicted.is_some() {
                            out.prompt_tokens = c.tokens_evaluated;
                            out.completion_tokens = c.tokens_predicted;
                        }
                        if let Some(t) = c.timings {
                            out.server_prompt_ms = t.prompt_ms;
                            out.server_predicted_ms = t.predicted_ms;
                        }
                        continue;
                    }
                    let Ok(c) = serde_json::from_str::<ChatChunk>(data) else { continue };
                    for ch in c.choices {
                        if let Some(d) = ch.delta.content {
                            if d.is_empty() {
                                continue;
                            }
                            if out.first_token_ms.is_none() {
                                out.first_token_ms = Some(t0.elapsed().as_millis() as u32);
                            }
                            text.push_str(&d);
                            on_delta(&d);
                        }
                    }
                    if let Some(u) = c.usage {
                        out.prompt_tokens = u.prompt_tokens;
                        out.completion_tokens = u.completion_tokens;
                    }
                    if let Some(t) = c.timings {
                        out.server_prompt_ms = t.prompt_ms;
                        out.server_predicted_ms = t.predicted_ms;
                    }
                }
            }
            out.text = text.trim().to_string();
            out.total_ms = t0.elapsed().as_millis() as u32;
            Ok(out)
        })
    }

    fn name(&self) -> String {
        format!("llama-server@{}", self.cfg.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_default() {
        let r = TranslateRequest {
            text: "Hello there".into(),
            src_lang: Lang::En,
            ..Default::default()
        };
        let p = build_prompt(&r);
        assert!(p.starts_with("将以下文本翻译为繁体中文"));
        assert!(p.ends_with("Hello there"));
        assert!(!p.contains("背景信息"));
        assert!(!p.contains("参考下面的翻译"));
    }

    #[test]
    fn prompt_with_context_and_glossary() {
        let r = TranslateRequest {
            text: "Exile, run!".into(),
            src_lang: Lang::En,
            context: vec![("Welcome.".into(), "歡迎。".into())],
            glossary: vec![GlossaryEntry {
                source: "Exile".into(),
                target: "流亡者".into(),
            }],
            ..Default::default()
        };
        let p = build_prompt(&r);
        assert!(p.starts_with("【背景信息】"));
        assert!(p.contains("Welcome. => 歡迎。"));
        assert!(p.contains("Exile 翻译成 流亡者"));
        assert!(p.contains("【待翻译文本】\nExile, run!"));
    }

    #[test]
    fn prompt_with_lookahead() {
        let r = TranslateRequest {
            text: "B".into(),
            src_lang: Lang::Ja,
            context: vec![("A".into(), "甲".into())],
            lookahead: vec!["C".into(), "D".into()],
            ..Default::default()
        };
        let p = build_prompt(&r);
        assert!(p.starts_with("【背景信息】\n以下是同一段影片中前面几句"));
        let ctx = p.find("A => 甲").unwrap();
        let la = p.find("不要翻译它们:\nC\nD\n").unwrap();
        let body = p.find("【待翻译文本】\nB").unwrap();
        assert!(ctx < la && la < body);
    }

    #[test]
    fn prompt_lookahead_without_context() {
        let r = TranslateRequest {
            text: "B".into(),
            src_lang: Lang::Ja,
            lookahead: vec!["C".into()],
            ..Default::default()
        };
        let p = build_prompt(&r);
        assert!(p.starts_with("【背景信息】\n以下是这句之后"));
        assert!(p.contains("【待翻译文本】\nB"));
    }

    #[test]
    fn id_slot_serialized_only_when_set() {
        let a = serde_json::to_string(&CompletionRequest { prompt: "x", stream: true, id_slot: None, temperature: 0.0, top_p: None, n_predict: 1, cache_prompt: true }).unwrap();
        assert!(!a.contains("id_slot"));
        let b = serde_json::to_string(&CompletionRequest { prompt: "x", stream: true, id_slot: Some(1), temperature: 0.0, top_p: None, n_predict: 1, cache_prompt: true }).unwrap();
        assert!(b.contains("\"id_slot\":1"));
    }

    #[test]
    fn style_by_model_name() {
        assert_eq!(PromptStyle::for_model("Sakura-7B-Qwen2.5-v1.0-IQ4_XS"), PromptStyle::Sakura);
        assert_eq!(PromptStyle::for_model("Hy-MT2-7B-Q4_K_M"), PromptStyle::HyMt2);
        let c = LlamaServerConfig::for_model("http://x", "Sakura-1.5B-Qwen2.5-v1.0-F16");
        assert_eq!(c.top_p, Some(0.3));
        assert_eq!(LlamaServerConfig::for_model("http://x", "Hy-MT2-1.8B-Q8_0").top_p, None);
    }

    #[test]
    fn sakura_messages() {
        let r = TranslateRequest {
            text: "今日は配信ありがとう".into(),
            src_lang: Lang::Ja,
            context: vec![("前".into(), "前".into())],
            ..Default::default()
        };
        let m = build_messages(&r, PromptStyle::Sakura);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].0, "system");
        assert_eq!(m[1].1, "将下面的日文文本翻译成中文：今日は配信ありがとう");
        let r2 = TranslateRequest {
            glossary: vec![GlossaryEntry {
                source: "配信".into(),
                target: "直播".into(),
            }],
            ..r
        };
        let m2 = build_messages(&r2, PromptStyle::Sakura);
        assert!(m2[1].1.starts_with("根据以下术语表"));
        assert!(m2[1].1.contains("配信->直播"));
    }

    #[test]
    fn other_styles_by_name_and_prompt() {
        assert_eq!(PromptStyle::for_model("Seed-X-PPO-7B-Q4_K_M"), PromptStyle::SeedX);
        assert_eq!(PromptStyle::for_model("LMT-60-4B-Q4_K_M"), PromptStyle::Lmt);
        assert_eq!(PromptStyle::for_model("TranslateGemma-12B-it-Q4_K_M"), PromptStyle::TranslateGemma);
        assert!(PromptStyle::SeedX.raw_completion());
        assert!(PromptStyle::TranslateGemma.raw_completion());
        assert!(!PromptStyle::Lmt.raw_completion());

        let r = TranslateRequest {
            text: "配信ありがとう".into(),
            src_lang: Lang::Ja,
            ..Default::default()
        };
        assert_eq!(
            build_seedx_prompt(&r),
            "Translate the following Japanese sentence into Chinese:\n配信ありがとう <zh>"
        );
        let lmt = build_lmt_prompt(&r);
        assert!(lmt.starts_with("Translate the following text from Japanese into Chinese:\nJapanese: 配信ありがとう"));
        assert!(lmt.ends_with("\nChinese:"));
        let tg = build_translategemma_prompt(&r);
        assert!(tg.starts_with("<start_of_turn>user\nYou are a professional Japanese (ja) to Chinese (zh-TW) translator."));
        assert!(tg.ends_with("Please translate the following Japanese text into Chinese:\n\n\n配信ありがとう<end_of_turn>\n<start_of_turn>model\n"));
        // greedy
        assert_eq!(LlamaServerConfig::for_model("http://x", "Seed-X-PPO-7B-Q4_K_M").temperature, 0.0);
    }
}
