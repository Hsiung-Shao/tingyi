//! 連續寫入單一 16 kHz 16-bit mono WAV(bench `--record-all` 用);結束時回填長度。

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::audio::TARGET_SAMPLE_RATE;

pub struct WavSink {
    file: BufWriter<File>,
    data_len: u32,
}

impl WavSink {
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).ok();
        }
        let f = File::create(path).with_context(|| format!("建立 {}", path.display()))?;
        let mut w = BufWriter::new(f);
        w.write_all(&header(0))?;
        Ok(Self { file: w, data_len: 0 })
    }

    pub fn write(&mut self, pcm: &[f32]) -> Result<()> {
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for &x in pcm {
            let v = (x.clamp(-1.0, 1.0) * 32767.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.data_len = self.data_len.saturating_add(bytes.len() as u32);
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.file.flush()?;
        let f = self.file.get_mut();
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(self.data_len))?;
        f.flush()?;
        Ok(())
    }
}

fn header(data_len: u32) -> [u8; 44] {
    let sr = TARGET_SAMPLE_RATE as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&sr.to_le_bytes());
    h[28..32].copy_from_slice(&(sr * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}
