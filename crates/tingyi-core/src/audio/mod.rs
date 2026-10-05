//! 音訊擷取:WASAPI loopback → 16 kHz 單聲道 f32。

pub mod capture;

/// 管線內部統一取樣率(SenseVoice、Silero VAD 共通)。
pub const TARGET_SAMPLE_RATE: i32 = 16_000;

/// 多聲道交錯 f32 → 單聲道(平均)。
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / channels;
    let inv = 1.0 / channels as f32;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * channels;
        let sum: f32 = interleaved[base..base + channels].iter().sum();
        out.push(sum * inv);
    }
    out
}

/// 小端 f32 位元組 → f32 樣本。
pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 段落音量正規化:把 RMS 拉到 `target_dbfs`,增益上限 `max_gain`(避免把底噪放大),
/// 已經夠大聲的段落不縮小,輸出 clip 在 ±1.0。
///
/// 小聲且連貫的語音在 SenseVoice 上辨識明顯變差(使用者實測),送辨識前先拉平音量。
pub fn normalize_rms(samples: &[f32], target_dbfs: f32, max_gain: f32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
    if rms < 1e-5 {
        return samples.to_vec(); // 幾乎靜音,不放大
    }
    let target = 10f32.powf(target_dbfs / 20.0);
    let gain = (target / rms).min(max_gain);
    if gain <= 1.0 {
        return samples.to_vec();
    }
    samples.iter().map(|x| (x * gain).clamp(-1.0, 1.0)).collect()
}

/// 段落 RMS(dBFS),診斷用。
pub fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -100.0;
    }
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
    20.0 * rms.max(1e-9).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_boosts_quiet_but_not_loud() {
        let quiet: Vec<f32> = (0..1600).map(|i| 0.01 * ((i as f32) * 0.1).sin()).collect();
        let out = normalize_rms(&quiet, -20.0, 8.0);
        let before = rms_dbfs(&quiet);
        let after = rms_dbfs(&out);
        assert!(after > before + 10.0, "before {before} after {after}");
        assert!(after <= -20.0 + 0.5);

        let loud: Vec<f32> = (0..1600).map(|i| 0.5 * ((i as f32) * 0.1).sin()).collect();
        assert_eq!(normalize_rms(&loud, -20.0, 8.0), loud);

        let silence = vec![0.0f32; 100];
        assert_eq!(normalize_rms(&silence, -20.0, 8.0), silence);

        // 增益上限:極小聲只放大 8 倍
        let tiny: Vec<f32> = (0..1600).map(|i| 0.0005 * ((i as f32) * 0.1).sin()).collect();
        let out = normalize_rms(&tiny, -20.0, 8.0);
        assert!((rms_dbfs(&out) - (rms_dbfs(&tiny) + 20.0 * 8f32.log10())).abs() < 0.5);
    }

    #[test]
    fn downmix_averages_channels() {
        let st = [1.0, 3.0, -1.0, 1.0];
        assert_eq!(downmix_to_mono(&st, 2), vec![2.0, 0.0]);
        assert_eq!(downmix_to_mono(&[0.5, 0.25], 1), vec![0.5, 0.25]);
    }

    #[test]
    fn bytes_roundtrip() {
        let v = [0.5f32, -0.25];
        let mut b = Vec::new();
        for x in v {
            b.extend_from_slice(&x.to_le_bytes());
        }
        assert_eq!(bytes_to_f32(&b), v);
    }
}
