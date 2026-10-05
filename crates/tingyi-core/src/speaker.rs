//! 說話者辨識:每段語音算聲紋 embedding(3D-Speaker CAM++,sherpa-onnx,CPU),
//! 與已知說話者比對;沒命中就新建「說話者 N」。支援改名與前後文規則。
//!
//! 限制:多人同時講話分不開;短段(< 1 s)不算,沿用上一位。

use anyhow::{anyhow, Result};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, SpeakerEmbeddingManager};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::audio::TARGET_SAMPLE_RATE;

#[derive(Debug, Clone)]
pub struct SpeakerConfig {
    pub model: PathBuf,
    /// 餘弦相似度門檻;高於此視為同一人。
    pub threshold: f32,
    /// 短於此的段落不算聲紋(沿用上一位)。
    pub min_audio_ms: u32,
    /// 每位說話者最多保留幾筆 embedding(滾動)。
    pub max_per_speaker: usize,
    pub num_threads: i32,
}

impl Default for SpeakerConfig {
    fn default() -> Self {
        Self {
            model: crate::paths::speaker_model(),
            threshold: 0.55,
            min_audio_ms: 1000,
            max_per_speaker: 20,
            num_threads: 1,
        }
    }
}

pub struct SpeakerIdentifier {
    cfg: SpeakerConfig,
    extractor: SpeakerEmbeddingExtractor,
    manager: SpeakerEmbeddingManager,
    /// 自己保存每人的 embedding,供改名時重建 manager。
    store: HashMap<String, Vec<Vec<f32>>>,
    order: Vec<String>,
    next_n: usize,
    last: Option<(String, Instant)>,
}

impl SpeakerIdentifier {
    pub fn new(cfg: SpeakerConfig) -> Result<Self> {
        if !cfg.model.exists() {
            return Err(anyhow!("找不到說話者模型:{}", cfg.model.display()));
        }
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(cfg.model.to_string_lossy().to_string()),
            num_threads: cfg.num_threads,
            debug: false,
            provider: Some("cpu".into()),
        })
        .ok_or_else(|| anyhow!("SpeakerEmbeddingExtractor::create 失敗"))?;
        let manager = SpeakerEmbeddingManager::create(extractor.dim())
            .ok_or_else(|| anyhow!("SpeakerEmbeddingManager::create 失敗"))?;
        Ok(Self {
            cfg,
            extractor,
            manager,
            store: HashMap::new(),
            order: Vec::new(),
            next_n: 1,
            last: None,
        })
    }

    /// 算一段 16 kHz 音訊的 embedding。
    pub fn embed(&self, pcm: &[f32]) -> Option<Vec<f32>> {
        let stream = self.extractor.create_stream()?;
        stream.accept_waveform(TARGET_SAMPLE_RATE, pcm);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return None;
        }
        self.extractor.compute(&stream)
    }

    /// 餘弦相似度(embedding 已由模型正規化,仍重算保險)。
    pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
    }

    /// 判斷這段是誰講的。`end_at` 為段落結束時間,供「短段沿用上一位」規則。
    pub fn identify(&mut self, pcm: &[f32], end_at: Instant) -> Option<String> {
        let audio_ms = pcm.len() as u64 * 1000 / TARGET_SAMPLE_RATE as u64;
        let short = audio_ms < self.cfg.min_audio_ms as u64;
        let recent = self
            .last
            .as_ref()
            .map(|(_, t)| end_at.saturating_duration_since(*t).as_millis() < 1500)
            .unwrap_or(false);
        if short {
            // 太短不算聲紋:剛有人講過就沿用,否則不標
            return if recent { self.last.as_ref().map(|(n, _)| n.clone()) } else { None };
        }
        let emb = self.embed(pcm)?;
        let best = self.manager.get_best_matches(&emb, 0.0, 1).into_iter().next();
        let name = match best {
            Some(m) if m.score >= self.cfg.threshold => m.name,
            Some(m)
                if audio_ms < 1500
                    && recent
                    && m.score >= self.cfg.threshold - 0.1
                    && self.last.as_ref().map(|(n, _)| n == &m.name).unwrap_or(false) =>
            {
                // 短段、分數接近門檻、而且就是上一位 → 沿用
                m.name
            }
            _ => {
                let n = format!("說話者 {}", self.next_n);
                self.next_n += 1;
                self.order.push(n.clone());
                n
            }
        };
        self.add_embedding(&name, emb);
        self.last = Some((name.clone(), end_at));
        Some(name)
    }

    fn add_embedding(&mut self, name: &str, emb: Vec<f32>) {
        let list = self.store.entry(name.to_string()).or_default();
        list.push(emb.clone());
        if list.len() > self.cfg.max_per_speaker {
            list.remove(0);
            // 超過上限:用滾動後的清單重建這個人
            let snapshot = list.clone();
            self.manager.remove(name);
            self.manager.add_list(name, &snapshot);
        } else {
            self.manager.add(name, &emb);
        }
    }

    /// 改名;舊名的所有 embedding 轉到新名下。新名已存在則合併。
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let to = to.trim();
        if to.is_empty() || from == to {
            return false;
        }
        let Some(embs) = self.store.remove(from) else { return false };
        self.manager.remove(from);
        let merged = self.store.entry(to.to_string()).or_default();
        merged.extend(embs);
        while merged.len() > self.cfg.max_per_speaker {
            merged.remove(0);
        }
        let snapshot = merged.clone();
        self.manager.remove(to);
        self.manager.add_list(to, &snapshot);
        if let Some(pos) = self.order.iter().position(|n| n == from) {
            if self.order.iter().any(|n| n == to) {
                self.order.remove(pos);
            } else {
                self.order[pos] = to.to_string();
            }
        }
        if let Some((n, _)) = &mut self.last {
            if n == from {
                *n = to.to_string();
            }
        }
        true
    }

    /// 已知說話者(依出現順序)。
    pub fn speakers(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn reset(&mut self) {
        for n in self.order.drain(..) {
            self.manager.remove(&n);
        }
        self.store.clear();
        self.next_n = 1;
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        assert!((SpeakerIdentifier::cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(SpeakerIdentifier::cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(SpeakerIdentifier::cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }
}
