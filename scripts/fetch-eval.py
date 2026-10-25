"""下載 CER 評測集:從 Hugging Face parquet 只讀需要的 row group(不整檔下載),
轉成 16 kHz 單聲道 16-bit wav,寫 manifest.tsv(wav 檔名、語言、參考文字、原始列號)。

用法(專案根目錄):
  python scripts/fetch-eval.py [--sets ja-rs ja-jsut ja-fleurs en-fleurs ko-fleurs] [--n 300] [--out <資料目錄>/eval]
輸出目錄預設為 $TINGYI_DATA_DIR/eval,沒設時用專案內 data/eval。

注意:ja-rs(ReazonSpeech 測試集)與 ReazonSpeech 模型同領域,只當參考,決定預設模型時以其他集為準。
資料只供本機評測,不入庫(data/ 已 gitignore)。
"""
import argparse
import io
import os
import sys
from math import gcd
from pathlib import Path

import fsspec
import numpy as np
import pyarrow.parquet as pq
import soundfile as sf
from scipy.signal import resample_poly

HF = "https://huggingface.co/datasets"
FLEURS = f"{HF}/google/fleurs/resolve/refs%2Fconvert%2Fparquet"

# 名稱 → (parquet 網址, 語言, 參考文字欄位)
SETS = {
    "ja-rs": (f"{HF}/japanese-asr/ja_asr.reazonspeech_test/resolve/main/data/test-00000-of-00002.parquet", "ja", "transcription"),
    "ja-jsut": (f"{HF}/japanese-asr/ja_asr.jsut_basic5000/resolve/main/data/test-00000-of-00004.parquet", "ja", "transcription"),
    "ja-fleurs": (f"{FLEURS}/ja_jp/test/0000.parquet", "ja", "raw_transcription"),
    "en-fleurs": (f"{FLEURS}/en_us/test/0000.parquet", "en", "raw_transcription"),
    "ko-fleurs": (f"{FLEURS}/ko_kr/test/0000.parquet", "ko", "raw_transcription"),
}
TARGET_SR = 16_000


def to_16k_mono(data: np.ndarray, sr: int) -> np.ndarray:
    if data.ndim > 1:
        data = data.mean(axis=1)
    if sr != TARGET_SR:
        g = gcd(sr, TARGET_SR)
        data = resample_poly(data, TARGET_SR // g, sr // g)
    return np.clip(data, -1.0, 1.0).astype(np.float32)


def clean_ref(text: str) -> str:
    return " ".join(str(text).replace("\t", " ").split())


def fetch_set(name: str, n: int, out_root: Path) -> int:
    url, lang, text_col = SETS[name]
    out = out_root / name
    out.mkdir(parents=True, exist_ok=True)
    manifest = out / "manifest.tsv"
    if manifest.exists() and sum(1 for _ in manifest.open(encoding="utf-8")) >= n:
        print(f"{name}:已有 {n} 句,略過")
        return 0
    rows = []
    with fsspec.open(url, "rb", block_size=4 * 1024 * 1024) as f:
        pf = pq.ParquetFile(f)
        cols = [c for c in pf.schema_arrow.names if c in ("audio", text_col)]
        print(f"{name}:{pf.metadata.num_rows} 列、{pf.num_row_groups} 個 row group,取前 {n} 列")
        idx = 0
        for g in range(pf.num_row_groups):
            table = pf.read_row_group(g, columns=cols).to_pylist()
            for r in table:
                ref = clean_ref(r[text_col])
                audio = r["audio"]
                if not ref or not audio or not audio.get("bytes"):
                    idx += 1
                    continue
                data, sr = sf.read(io.BytesIO(audio["bytes"]), dtype="float32")
                wav_name = f"{len(rows):04d}.wav"
                sf.write(out / wav_name, to_16k_mono(data, sr), TARGET_SR, subtype="PCM_16")
                rows.append((wav_name, lang, ref, idx))
                idx += 1
                if len(rows) >= n:
                    break
            print(f"  row group {g}:累計 {len(rows)} 句")
            if len(rows) >= n:
                break
    with manifest.open("w", encoding="utf-8", newline="\n") as m:
        for wav_name, lg, ref, i in rows:
            m.write(f"{wav_name}\t{lg}\t{ref}\t{i}\n")
    print(f"{name}:寫入 {len(rows)} 句 → {manifest}")
    return 0 if len(rows) >= n else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--sets", nargs="+", default=list(SETS))
    ap.add_argument("--n", type=int, default=300)
    default_root = Path(os.environ["TINGYI_DATA_DIR"]) if os.environ.get("TINGYI_DATA_DIR") else Path(__file__).resolve().parents[1] / "data"
    ap.add_argument("--out", type=Path, default=default_root / "eval")
    a = ap.parse_args()
    unknown = [s for s in a.sets if s not in SETS]
    if unknown:
        print(f"未知的評測集:{unknown};可用 {list(SETS)}", file=sys.stderr)
        return 2
    return max(fetch_set(s, a.n, a.out) for s in a.sets)


if __name__ == "__main__":
    sys.exit(main())
