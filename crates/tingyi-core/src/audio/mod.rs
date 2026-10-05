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

#[cfg(test)]
mod tests {
    use super::*;

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
