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
    /// 餘弦相似度 ≥ 此值 → 歸到該說話者。
    pub threshold: f32,
    /// 相似度 < 此值才新建說話者;介於兩者之間歸到最像的既有說話者(遲滯,避免同一人被拆成多位)。
    pub new_below: f32,
    /// 兩位說話者的平均聲紋相似度 ≥ 此值 → 自動合併。
    pub merge_above: f32,
    /// 短於此的段落不算聲紋(沿用上一位)。
    pub min_audio_ms: u32,
    /// 短於此的段落不新建說話者(短段聲紋不可靠)。
    pub new_speaker_min_ms: u32,
    /// 多人偵測的小窗長度(毫秒);段落 ≥ 2 窗才做。
    pub window_ms: u32,
    /// 小窗命中既有說話者的門檻。
    pub window_threshold: f32,
    /// 每位說話者最多保留幾筆 embedding(滾動)。
    pub max_per_speaker: usize,
    pub num_threads: i32,
}

impl Default for SpeakerConfig {
    fn default() -> Self {
        Self {
            model: crate::paths::speaker_model(),
            // 2026-10-06 實測(英文對談 4 人、單人英文):同一人 ≥ 0.61(乾淨 ≥ 0.76),
            // 不同人多在 0.1–0.5、交接段偶爾 0.63。
            threshold: 0.60,
            new_below: 0.50,
            merge_above: 0.75,
            min_audio_ms: 1000,
            new_speaker_min_ms: 2000,
            window_ms: 1500,
            window_threshold: 0.55,
            max_per_speaker: 20,
            num_threads: 1,
        }
    }
}

/// 多人同時/交錯講話的段落標籤(不是真正的說話者,不會進清單)。
pub const MULTI_SPEAKER: &str = "多人";

pub struct SpeakerIdentifier {
    cfg: SpeakerConfig,
    extractor: SpeakerEmbeddingExtractor,
    manager: SpeakerEmbeddingManager,
    /// 自己保存每人的 embedding,供改名時重建 manager。
    store: HashMap<String, Vec<Vec<f32>>>,
    order: Vec<String>,
    next_n: usize,
    last: Option<(String, Instant)>,
    /// 自動合併紀錄(from → into),由管線取走後送事件給前端。
    merges: Vec<(String, String)>,
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
            merges: Vec::new(),
        })
    }

    /// 取走自動合併紀錄。
    pub fn take_merges(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.merges)
    }

    fn centroid(embs: &[Vec<f32>]) -> Vec<f32> {
        let dim = embs.first().map(|e| e.len()).unwrap_or(0);
        let mut c = vec![0f32; dim];
        for e in embs {
            for (i, v) in e.iter().enumerate() {
                c[i] += v;
            }
        }
        let n = embs.len().max(1) as f32;
        c.iter_mut().for_each(|v| *v /= n);
        c
    }

    /// 兩位說話者平均聲紋太像就合併(後出現的併入先出現的)。
    fn auto_merge(&mut self) {
        let names: Vec<String> = self.order.clone();
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                let (a, b) = (&names[i], &names[j]);
                let (Some(ea), Some(eb)) = (self.store.get(a), self.store.get(b)) else { continue };
                if ea.len() < 3 || eb.len() < 3 {
                    continue; // 各至少三段再判斷,避免剛出現的人被誤併
                }
                let sim = Self::cosine(&Self::centroid(ea), &Self::centroid(eb));
                if sim >= self.cfg.merge_above {
                    let (from, into) = (b.clone(), a.clone());
                    if self.rename(&from, &into) {
                        self.merges.push((from, into));
                    }
                    return; // 一次合併一組,下次再看
                }
            }
        }
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
        // 各人分數 = max(與該人任一筆的最高相似度, 與該人平均聲紋的相似度)
        let mut scores: Vec<(String, f32)> = self
            .store
            .iter()
            .map(|(name, embs)| {
                let max_s = embs.iter().map(|e| Self::cosine(e, &emb)).fold(f32::MIN, f32::max);
                let cen_s = Self::cosine(&Self::centroid(embs), &emb);
                (name.clone(), max_s.max(cen_s))
            })
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let best = scores.first().cloned();
        let second = scores.get(1).cloned();

        // 多人偵測 1:小窗分別命中不同的已知說話者
        if self.windows_hit_multiple(pcm) {
            self.last = None; // 多人段不當作「剛講完的人」
            return Some(MULTI_SPEAKER.to_string());
        }
        // 多人偵測 2:整段對兩位以上都只有中等相似(混在一起的聲音)
        if let (Some((_, s1)), Some((_, s2))) = (&best, &second) {
            if *s1 < self.cfg.threshold && *s1 >= self.cfg.new_below - 0.1 && *s2 >= self.cfg.new_below - 0.1 {
                self.last = None;
                return Some(MULTI_SPEAKER.to_string());
            }
        }

        let last_name = self.last.as_ref().map(|(n, _)| n.clone());
        let name = match best {
            Some((n, s)) if s >= self.cfg.threshold => n,
            // 遲滯區:只有「最像的就是剛講完的那位」才沿用(同一人語氣變化)
            Some((n, s)) if s >= self.cfg.new_below && recent && last_name.as_deref() == Some(n.as_str()) => n,
            // 太短的段落不新建:剛有人講過就沿用,否則不標
            _ if audio_ms < self.cfg.new_speaker_min_ms as u64 => {
                return if recent { last_name } else { None };
            }
            _ => self.new_speaker(),
        };
        self.add_embedding(&name, emb);
        self.last = Some((name.clone(), end_at));
        self.auto_merge();
        // 合併可能把 name 改掉
        let name = self.last.as_ref().map(|(n, _)| n.clone()).unwrap_or(name);
        Some(name)
    }

    /// 只比對、不學習:講話中的即時預覽用,回傳最像的已知說話者(需 ≥ 門檻),否則 None。
    pub fn peek(&self, pcm: &[f32]) -> Option<String> {
        if self.store.is_empty() || pcm.len() < (self.cfg.min_audio_ms as usize) * TARGET_SAMPLE_RATE as usize / 1000 {
            return None;
        }
        let emb = self.embed(pcm)?;
        let mut best: Option<(&String, f32)> = None;
        for (name, embs) in &self.store {
            let max_s = embs.iter().map(|e| Self::cosine(e, &emb)).fold(f32::MIN, f32::max);
            let cen_s = Self::cosine(&Self::centroid(embs), &emb);
            let s = max_s.max(cen_s);
            if best.map(|(_, b)| s > b).unwrap_or(true) {
                best = Some((name, s));
            }
        }
        best.filter(|(_, s)| *s >= self.cfg.threshold).map(|(n, _)| n.clone())
    }

    /// 把段落切成 `window_ms` 的小窗各算聲紋;若命中兩位以上不同的已知說話者 → 多人。
    fn windows_hit_multiple(&self, pcm: &[f32]) -> bool {
        if self.store.len() < 2 {
            return false;
        }
        let win = (self.cfg.window_ms as usize) * TARGET_SAMPLE_RATE as usize / 1000;
        if pcm.len() < win * 2 {
            return false;
        }
        let mut hits: Vec<String> = Vec::new();
        let mut start = 0;
        while start + win <= pcm.len() {
            if let Some(e) = self.embed(&pcm[start..start + win]) {
                let mut best: Option<(&String, f32)> = None;
                for (name, embs) in &self.store {
                    let s = embs.iter().map(|x| Self::cosine(x, &e)).fold(f32::MIN, f32::max);
                    if best.map(|(_, b)| s > b).unwrap_or(true) {
                        best = Some((name, s));
                    }
                }
                if let Some((n, s)) = best {
                    if s >= self.cfg.window_threshold && !hits.contains(n) {
                        hits.push(n.clone());
                    }
                }
            }
            start += win;
        }
        hits.len() >= 2
    }

    fn new_speaker(&mut self) -> String {
        let n = format!("說話者 {}", self.next_n);
        self.next_n += 1;
        self.order.push(n.clone());
        n
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
    fn centroid_is_mean() {
        let c = SpeakerIdentifier::centroid(&[vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(c, vec![0.5, 0.5]);
        assert!(SpeakerIdentifier::centroid(&[]).is_empty());
    }

    #[test]
    fn cosine_basics() {
        assert!((SpeakerIdentifier::cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(SpeakerIdentifier::cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(SpeakerIdentifier::cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }
}
