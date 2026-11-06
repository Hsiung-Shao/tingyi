"""產生自動更新用的 latest.json 與要上傳到 GitHub Release 的檔案。

`pnpm tauri build`(需環境變數 TAURI_SIGNING_PRIVATE_KEY 指向簽章私鑰)會在
target/release/bundle/nsis/ 產出 `聽譯_<版本>_x64-setup.exe` 與 `.sig`。這支腳本把它們
改名成 Release 用的 ASCII 檔名 `tingyi_<版本>_x64-setup.exe`(與既有 Release 一致),
並寫出 app 檢查更新時讀的 latest.json。三個檔都要上傳到 v<版本> 的 Release。

用法:
  python scripts/make-update-manifest.py --version 0.1.2 --notes-file notes.md
  python scripts/make-update-manifest.py --version 0.1.99 --base-url http://127.0.0.1:8000   # 本機實跑
"""
import argparse
import datetime
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPO = "Hsiung-Shao/tingyi"


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="產生 latest.json 與 Release 上傳檔")
    ap.add_argument("--version", required=True)
    ap.add_argument("--bundle-dir", default=str(ROOT / "target" / "release" / "bundle" / "nsis"))
    ap.add_argument("--out-dir", default=str(ROOT / "target" / "release" / "update"))
    ap.add_argument("--notes-file", help="更新說明(顯示在 app 的「關於與更新」)")
    ap.add_argument("--base-url", help=f"安裝檔下載網址前綴;預設 https://github.com/{REPO}/releases/download/v<版本>")
    a = ap.parse_args(argv)

    v = a.version
    src = Path(a.bundle_dir) / f"聽譯_{v}_x64-setup.exe"
    sig = src.with_name(src.name + ".sig")
    if not src.is_file():
        print(f"找不到安裝檔:{src}", file=sys.stderr)
        return 1
    if not sig.is_file():
        # 沒有簽章的更新 app 會拒絕安裝;多半是建置時沒設 TAURI_SIGNING_PRIVATE_KEY
        print(f"找不到簽章檔 .sig:{sig}(建置時要設 TAURI_SIGNING_PRIVATE_KEY)", file=sys.stderr)
        return 1

    out = Path(a.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    name = f"tingyi_{v}_x64-setup.exe"
    shutil.copyfile(src, out / name)
    shutil.copyfile(sig, out / (name + ".sig"))
    base = (a.base_url or f"https://github.com/{REPO}/releases/download/v{v}").rstrip("/")
    notes = Path(a.notes_file).read_text(encoding="utf-8") if a.notes_file else ""
    manifest = {
        "version": v,
        "notes": notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": {
            "windows-x86_64": {
                "signature": sig.read_text(encoding="utf-8"),
                "url": f"{base}/{name}",
            }
        },
    }
    (out / "latest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"已輸出 {out}:{name}、{name}.sig、latest.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
