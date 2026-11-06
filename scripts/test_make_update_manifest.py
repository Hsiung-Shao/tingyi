import importlib.util
import json
from datetime import datetime
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "make-update-manifest.py"
VERSION = "0.1.4"
SIG_TEXT = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQpSVVRhYmNkZWY=\nQUJDREVGR0hJSktMTU5PUA==\n"
NOTES = "修正:辨識中不再彈出更新通知\n新增:自動檢查更新"


def load_module():
    spec = importlib.util.spec_from_file_location("make_update_manifest", SCRIPT)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def prepare(tmp_path, with_sig=True):
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    exe = bundle / f"聽譯_{VERSION}_x64-setup.exe"
    exe.write_bytes(b"\x4d\x5a\x90\x00fake-installer")
    if with_sig:
        (bundle / f"聽譯_{VERSION}_x64-setup.exe.sig").write_bytes(SIG_TEXT.encode("utf-8"))
    notes = tmp_path / "notes.md"
    notes.write_text(NOTES, encoding="utf-8")
    return bundle, tmp_path / "out", notes, exe


def run(mod, bundle, out, notes=None, base_url=None):
    argv = ["--version", VERSION, "--bundle-dir", str(bundle), "--out-dir", str(out)]
    if notes is not None:
        argv += ["--notes-file", str(notes)]
    if base_url is not None:
        argv += ["--base-url", base_url]
    return mod.main(argv)


def test_M1_產出安裝檔簽章與_latest_json(tmp_path):
    """M1:成功時輸出三個檔,json 欄位與簽章原文一致"""
    mod = load_module()
    bundle, out, notes, exe = prepare(tmp_path)
    assert run(mod, bundle, out, notes) == 0

    name = f"tingyi_{VERSION}_x64-setup.exe"
    assert (out / name).read_bytes() == exe.read_bytes()
    assert (out / f"{name}.sig").read_bytes() == SIG_TEXT.encode("utf-8")
    assert (out / "latest.json").exists()

    data = json.loads((out / "latest.json").read_text(encoding="utf-8"))
    assert data["version"] == VERSION
    assert data["notes"] == NOTES
    plat = data["platforms"]["windows-x86_64"]
    assert plat["signature"] == SIG_TEXT
    assert plat["url"].endswith(f"/{name}")
    assert plat["url"] == f"https://github.com/Hsiung-Shao/tingyi/releases/download/v{VERSION}/{name}"
    datetime.fromisoformat(data["pub_date"].replace("Z", "+00:00"))


def test_M1_省略_notes_時為空字串(tmp_path):
    """M1:不給 --notes-file 時 notes 為空字串"""
    mod = load_module()
    bundle, out, _notes, _exe = prepare(tmp_path)
    assert run(mod, bundle, out) == 0
    data = json.loads((out / "latest.json").read_text(encoding="utf-8"))
    assert data["notes"] == ""


def test_M1_base_url_可覆寫(tmp_path):
    """M1:帶 --base-url 時 url 使用該前綴"""
    mod = load_module()
    bundle, out, notes, _exe = prepare(tmp_path)
    assert run(mod, bundle, out, notes, base_url="http://127.0.0.1:8000") == 0
    data = json.loads((out / "latest.json").read_text(encoding="utf-8"))
    url = data["platforms"]["windows-x86_64"]["url"]
    assert url == f"http://127.0.0.1:8000/tingyi_{VERSION}_x64-setup.exe"


def test_M2_缺_sig_時失敗且不產生_latest_json(tmp_path, capsys):
    """M2:沒有 .sig 回非 0、stderr 提到 .sig、不產生 latest.json"""
    mod = load_module()
    bundle, out, notes, _exe = prepare(tmp_path, with_sig=False)
    code = run(mod, bundle, out, notes)
    assert code != 0
    assert ".sig" in capsys.readouterr().err
    assert not (out / "latest.json").exists()
