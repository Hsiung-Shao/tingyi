//! 翻譯:統一介面 `Translator` + `LlamaServerTranslator`(OpenAI 相容串流端點)。
//!
//! 本機 llama-server 與雲端 OpenAI 相容服務共用同一套程式碼。
//! 提示依 Hy-MT2 模型卡(簡體原文),目標語言「繁體中文」。

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
    if !req.context.is_empty() {
        p.push_str("【背景信息】\n以下是同一段影片中前面几句的原文与译文,请保持称谓、语气与用词一致:\n");
        for (s, t) in &req.context {
            p.push_str(s);
            p.push_str(" => ");
            p.push_str(t);
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
    if req.context.is_empty() {
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
    temperature: f32,
    max_tokens: u32,
    cache_prompt: bool,
    stream_options: StreamOptions,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
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
            let prompt = build_prompt(&req);
            let body = ChatRequest {
                model: &self.cfg.model,
                messages: vec![ChatMessage {
                    role: "user",
                    content: &prompt,
                }],
                stream: true,
                temperature: self.cfg.temperature,
                max_tokens: self.cfg.max_tokens,
                cache_prompt: true,
                stream_options: StreamOptions { include_usage: true },
            };
            let url = format!(
                "{}/v1/chat/completions",
                self.cfg.base_url.trim_end_matches('/')
            );
            let mut rb = self.client.post(&url).json(&body);
            if let Some(k) = &self.cfg.api_key {
                rb = rb.bearer_auth(k);
            }
            let resp = rb.send().await.context("POST /v1/chat/completions")?;
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
        };
        let p = build_prompt(&r);
        assert!(p.starts_with("【背景信息】"));
        assert!(p.contains("Welcome. => 歡迎。"));
        assert!(p.contains("Exile 翻译成 流亡者"));
        assert!(p.contains("【待翻译文本】\nExile, run!"));
    }
}
