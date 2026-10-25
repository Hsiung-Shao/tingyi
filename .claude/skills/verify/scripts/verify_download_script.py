"""下載腳本 scripts/download-models.ps1 的實跑驗證(真網路):

A. 乾淨資料目錄下載必要項目 + 日文/英文專用模型壓縮包 → exit 0、無 .part 殘留、壓縮包已解壓並刪除;
   Python 獨立重算每個下載檔的 SHA256,必須是腳本寫死的值;解壓出的 int8 模型要與 app 內下載規格
   (crates/tingyi-core/src/model_download.rs)的 SHA256 相同。
B. 同一目錄加 -Verify → exit 0、「全部相符」。
C. 竄改 silero_vad.onnx 一個位元組後 -Verify → exit 非 0 且點名該檔(之後還原)。
D. (選用 --existing)對既有資料目錄的全部選配項目跑 -Verify;先 -ListOnly,有「待下載」就不跑(免得下載進使用者目錄)。

用法:python .claude/skills/verify/scripts/verify_download_script.py [--existing D:\\codeproject\\translate\\data]
腳本最後會把 llama-server 複製到本 worktree 的 src-tauri\\binaries(已 gitignore,與平常執行相同)。
"""
import argparse
import datetime
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from verify_audio_source import ROOT, SKILL, Run  # noqa: E402

PS1 = ROOT / "scripts" / "download-models.ps1"
MODEL_SPEC = ROOT / "crates" / "tingyi-core" / "src" / "model_download.rs"
CLEAN = ["-WithReazonSpeech", "-WithParakeet"]
ALL = ["-With7B", "-With1_8B_Q4", "-WithWhisper", "-WithSakura", "-WithCandidates", "-WithReazonSpeech", "-WithParakeet"]
RS_DIR = "sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01"
PK_DIR = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        while chunk := f.read(8 << 20):
            h.update(chunk)
    return h.hexdigest()


def hex64(path: Path) -> set:
    return set(re.findall(r"['\"]([0-9a-f]{64})['\"]", path.read_text(encoding="utf-8-sig")))


def ps(run: Run, name: str, args: list) -> tuple:
    cmd = ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(PS1), *args]
    run.log(f"執行 {name}:{' '.join(args)}")
    p = subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    text = p.stdout.decode("cp950", errors="replace")  # Windows PowerShell 5.1 依主控台字碼頁輸出
    (run.evidence / f"{name}.log").write_text(text, encoding="utf-8")
    run.log(f"{name} exit {p.returncode}")
    return p.returncode, text


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--existing", help="既有資料目錄,對全部選配項目跑 -Verify(不下載)")
    a = ap.parse_args()
    evidence = SKILL / "evidence" / (datetime.datetime.now().strftime("%Y%m%d-%H%M%S") + "-dlscript")
    evidence.mkdir(parents=True)
    run = Run(evidence)
    data = evidence / "data"
    ok = False
    try:
        expected = hex64(PS1)
        app_spec = hex64(MODEL_SPEC)
        run.log(f"腳本寫死 {len(expected)} 個 SHA256;app 下載規格 {len(app_spec)} 個")

        code, out = ps(run, "A-clean", ["-DataDir", str(data), *CLEAN])
        run.check("A 乾淨目錄下載 exit 0", code == 0, out.strip().splitlines()[-1] if out.strip() else "")
        files = sorted(p for p in data.rglob("*") if p.is_file())
        run.check("A 沒有 .part 殘留", not [p for p in files if p.suffix == ".part"])
        run.check("A 壓縮包解壓後已刪除", not [p for p in files if p.name.endswith(".tar.bz2")])
        run.check("A 專用模型已解壓(tokens.txt 存在)",
                  all((data / "models" / d / "tokens.txt").exists() for d in (RS_DIR, PK_DIR)))
        run.check("A llama-server 已解壓", (data / "bin" / "llama" / "llama-server.exe").exists())
        top = [p for p in files if p.parent in (data / "models", data / "bin")
               or p.parent == data / "models" / "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"]
        hashes = {str(p.relative_to(data)): sha256(p) for p in top}
        run.result["downloaded"] = hashes
        wrong = {k: v for k, v in hashes.items() if v not in expected}
        run.check(f"A 下載的 {len(hashes)} 個檔案 SHA256 都是腳本寫死的值", bool(hashes) and not wrong, json.dumps(wrong))
        int8 = [p for d in (RS_DIR, PK_DIR) for p in (data / "models" / d).iterdir()
                if p.name.endswith(".int8.onnx") or p.name == "tokens.txt"]
        cross = {str(p.relative_to(data)): sha256(p) for p in int8}
        bad_cross = {k: v for k, v in cross.items() if v not in app_spec}
        run.check(f"A 解壓出的 {len(cross)} 個專用模型檔與 app 內下載規格相同", len(cross) == 8 and not bad_cross,
                  json.dumps(bad_cross))

        code, out = ps(run, "B-verify", ["-DataDir", str(data), "-Verify", *CLEAN])
        run.check("B -Verify exit 0 且全部相符", code == 0 and "已存在的檔案 SHA256 全部相符" in out)

        vad = data / "models" / "silero_vad.onnx"
        orig = vad.read_bytes()
        vad.write_bytes(orig[:-1] + bytes([orig[-1] ^ 0xFF]))
        try:
            code, out = ps(run, "C-tamper", ["-DataDir", str(data), "-Verify"])
        finally:
            vad.write_bytes(orig)
        run.check("C 竄改後 -Verify 失敗且點名檔案", code != 0 and "SHA256 不符" in out and "silero_vad.onnx" in out, f"exit {code}")

        if a.existing:
            code, out = ps(run, "D-list", ["-DataDir", a.existing, "-Verify", "-ListOnly", *ALL])
            if run.check("D 既有目錄沒有待下載項目", code == 0 and "待下載" not in out):
                code, out = ps(run, "D-existing", ["-DataDir", a.existing, "-Verify", *ALL])
                run.check("D 既有目錄全部選配項目 SHA256 相符", code == 0 and "已存在的檔案 SHA256 全部相符" in out)
        ok = all(c["ok"] for c in run.result["checks"])
        return 0 if ok else 1
    except Exception as e:  # 驗證腳本本身出錯也要留下紀錄並判失敗
        run.check("驗證流程未拋例外", False, repr(e))
        return 1
    finally:
        if data.exists():
            listing = [f"{p.relative_to(data)}\t{p.stat().st_size}" for p in sorted(data.rglob("*")) if p.is_file()]
            (evidence / "data-files.txt").write_text("\n".join(listing), encoding="utf-8")
            shutil.rmtree(data, ignore_errors=True)  # 約 3.4 GB,只留清單
        (evidence / "result.json").write_text(json.dumps(run.result, ensure_ascii=False, indent=2), encoding="utf-8")
        code = 0 if ok else 1
        (evidence / "exit_code.txt").write_text(str(code), encoding="utf-8")
        run.log(f"結束,exit {code};證據 {evidence}")
        run.log_f.close()


if __name__ == "__main__":
    sys.exit(main())
