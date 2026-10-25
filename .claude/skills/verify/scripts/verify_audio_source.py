"""實跑驗證:指定程式音訊來源與執行中切換(聽譯 tingyi)。

流程(全程只在頁面內用 CDP 操作,不送 OS 鍵鼠輸入):
1. 拒絕雙開:1420 / 9339 埠或 tingyi.exe 已在跑就失敗。
2. `pnpm tauri dev`(WebView2 開遠端除錯埠),等主視窗「開始」可按。
3. 自己啟動兩個供音程式:TTS(powershell.exe 唸英文)與靜音播放器(powershell.exe 循環播放靜音 wav)。
4. 來源選 TTS → 開始 → 應出現與 TTS 句子相符的字幕。
5. 執行中切到靜音播放器 → 12 秒內不得新增字幕(TTS 仍在唸)。
6. 切回 TTS → 應再出現相符字幕(正向對照,證明第 5 步不是因為 TTS 停了)。
7. 停止、截圖、只關閉自己啟動的行程。證據寫到 evidence/<時間>/,run.log + exit_code.txt + 截圖 + result.json。

用法(在專案根目錄):
  python .claude/skills/verify/scripts/verify_audio_source.py [--data-dir D:\\codeproject\\translate\\data]
"""
import argparse
import asyncio
import datetime
import json
import os
import re
import socket
import subprocess
import sys
import time
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cdp import connect, find_page  # noqa: E402

ROOT = Path(__file__).resolve().parents[4]
SKILL = ROOT / ".claude" / "skills" / "verify"
CDP_PORT = 9339  # 9222 常被 Chrome 除錯埠占用
DEV_PORT = 1420
URL_PREFIX = f"http://localhost:{DEV_PORT}/"
NO_WINDOW = 0x08000000  # CREATE_NO_WINDOW

TTS_LINES = [
    "The weather today is sunny with a light breeze from the west.",
    "Please remember to check the build settings before the next patch.",
    "Our team finished the final boss fight after three long attempts.",
    "The library will close early on Friday because of the holiday.",
    "She bought fresh bread and green apples at the morning market.",
    "Turn left at the second traffic light and the station is on your right.",
    "The new graphics card runs much cooler than the old one.",
    "We should upgrade the server before more players join tonight.",
    "My brother is learning to play the guitar during summer vacation.",
    "The museum has a special exhibition about ancient ships.",
    "Remember to drink water and take a short break every hour.",
    "The train to the airport leaves every fifteen minutes.",
    "This recipe needs two eggs, some butter, and a cup of sugar.",
    "The pilot announced that we will land about ten minutes early.",
    "Our cat sleeps on the warm windowsill every afternoon.",
    "The students planted small trees around the school garden.",
    "Please send me the report before the meeting on Monday.",
    "A strong wind knocked down several branches near the river.",
    "The concert tickets sold out within the first hour.",
    "He fixed the broken chair with a few nails and some glue.",
    "The children built a huge sandcastle at the beach yesterday.",
    "Our neighbors are moving to a bigger house next month.",
    "The coffee shop on the corner makes excellent pancakes.",
    "Scientists discovered a new species of frog in the rainforest.",
]

JS_HELPERS = r"""
window.__v = {
  btn: (text) => [...document.querySelectorAll('button')].find(b => b.textContent.trim() === text) || null,
  sentences: () => [...document.querySelectorAll('[data-sentence-id]')].map(e => ({
    id: +e.dataset.sentenceId, lang: e.dataset.lang,
    source: (e.querySelector('[data-role=source]') || {}).textContent || ''
  })),
  select: () => document.querySelector('select[aria-label="音訊來源"]'),
  options: () => { const s = window.__v.select(); return s ? [...s.options].map(o => o.value) : []; },
  choose: (value) => {
    const s = window.__v.select();
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, value);
    s.dispatchEvent(new Event('change', { bubbles: true }));
    return s.value;
  },
  setChecked: (labelText, checked) => {
    const l = [...document.querySelectorAll('label')].find(x => x.textContent.includes(labelText));
    const i = l && l.querySelector('input');
    if (!i) return null;
    if (i.checked !== checked) i.click();
    return i.checked;
  },
  state: () => window.__TAURI_INTERNALS__.invoke('get_audio_source'),
};
true
"""


class Run:
    def __init__(self, evidence: Path):
        self.evidence = evidence
        self.log_f = open(evidence / "run.log", "w", encoding="utf-8")
        self.result = {"checks": []}

    def log(self, msg: str):
        line = f"{datetime.datetime.now():%H:%M:%S.%f}"[:-3] + f" {msg}"
        print(line, flush=True)
        self.log_f.write(line + "\n")
        self.log_f.flush()

    def check(self, name: str, ok: bool, detail: str = ""):
        self.result["checks"].append({"name": name, "ok": ok, "detail": detail})
        self.log(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}")
        return ok


def port_in_use(port: int) -> bool:
    with socket.socket() as s:
        s.settimeout(0.5)
        return s.connect_ex(("127.0.0.1", port)) == 0


def image_pids(image: str) -> set:
    out = subprocess.run(["tasklist", "/FI", f"IMAGENAME eq {image}", "/FO", "CSV", "/NH"],
                         capture_output=True, text=True, encoding="cp950", errors="replace").stdout
    return {int(m.group(1)) for m in re.finditer(r'^"[^"]+","(\d+)"', out, re.M)}


def pid_alive(pid: int) -> bool:
    out = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
                         capture_output=True, text=True, encoding="cp950", errors="replace").stdout
    return f'"{pid}"' in out


def kill_tree(run: Run, pid: int, label: str):
    if not pid_alive(pid):
        return
    subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
    time.sleep(0.5)
    run.log(f"關閉 {label} pid {pid}:{'仍存活!' if pid_alive(pid) else '已結束'}")


def words(text: str) -> set:
    return {w for w in re.findall(r"[a-z]+", text.lower()) if len(w) >= 3}


LINE_WORDS = [words(x) for x in TTS_LINES]


def matches_tts(source: str) -> bool:
    w = words(source)
    if len(w) < 2:
        return False
    return max(len(w & lw) for lw in LINE_WORDS) / len(w) >= 0.5


def write_silent_wav(path: Path, seconds: int = 2):
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(b"\x00\x00" * 16000 * seconds)


def screenshot(run: Run, name: str):
    out = run.evidence / name
    r = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                        str(ROOT / "scripts" / "shot-windows.ps1"), "-OutDir", str(out)],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    run.log(f"截圖 {name}:exit {r.returncode} {len(list(out.glob('*.png'))) if out.exists() else 0} 張")


async def wait_for(pred, timeout: float, interval: float = 0.5):
    t0 = time.monotonic()
    while time.monotonic() - t0 < timeout:
        v = await pred()
        if v:
            return v
        await asyncio.sleep(interval)
    return None


async def drive(run: Run, tts_pid: int, silent_pid: int) -> bool:
    ws, page = await connect(CDP_PORT, URL_PREFIX)
    try:
        async def js(expr, timeout=30.0):
            await page.eval(JS_HELPERS)  # 每次重注入:Vite HMR 重新載入會清掉
            return await page.eval(expr, timeout)

        async def sentences():
            return await js("window.__v.sentences()")

        ready = await wait_for(lambda: js("(() => { const b = window.__v.btn('開始'); return !!b && !b.disabled; })()"), 180)
        if not run.check("主視窗就緒(開始可按)", bool(ready)):
            return False
        translate_on = await js("window.__v.setChecked('啟用本機翻譯', false)")
        run.log(f"翻譯勾選狀態:{translate_on}(只驗擷取,不啟動 llama-server)")

        want = {f"app:{tts_pid}", f"app:{silent_pid}"}

        async def listed():
            await js("(() => { const b = document.querySelector('button[aria-label=\"重新整理程式清單\"]'); b && b.click(); return true; })()")
            await asyncio.sleep(1.5)
            return want <= set(await js("window.__v.options()"))
        ok = await wait_for(listed, 30, 0.5)
        if not run.check("程式清單列出 TTS 與靜音播放器", bool(ok), str(await js("window.__v.options()"))):
            return False

        async def choose(pid: int, label: str) -> bool:
            await js(f"window.__v.choose('app:{pid}')")
            st = await wait_for(lambda: js(f"window.__v.state().then(s => s.source.kind === 'app' && s.source.pid === {pid} ? s : null)"), 5)
            return run.check(f"來源切到{label}", bool(st), json.dumps(st, ensure_ascii=False))

        if not await choose(tts_pid, " TTS"):
            return False
        await js("window.__v.btn('開始').click(), true")
        running = await wait_for(lambda: js("!!window.__v.btn('停止')"), 120)
        if not run.check("開始後進入執行中", bool(running)):
            return False

        # 1) 指定 TTS:應出現相符字幕
        async def tts_hits(min_id=0):
            s = await sentences()
            hits = [x for x in s if x["id"] > min_id and matches_tts(x["source"])]
            return hits if hits else None
        hits = await wait_for(tts_hits, 45, 1.0)
        run.result["phase1"] = hits
        if not run.check("指定 TTS 時出現 TTS 字幕", bool(hits), json.dumps(hits, ensure_ascii=False)[:300]):
            return False
        screenshot(run, "1-tts")

        # 2) 執行中切到靜音播放器:不得新增字幕
        if not await choose(silent_pid, "靜音播放器"):
            return False
        await asyncio.sleep(3)  # 切換前已在辨識/合併中的句子可能晚到
        before = await sentences()
        await asyncio.sleep(12)
        after = await sentences()
        new = [x for x in after if x["id"] > max([y["id"] for y in before], default=0)]
        run.result["phase2_new"] = new
        tts_alive = pid_alive(tts_pid)
        run.check("負向對照期間 TTS 仍在唸", tts_alive)
        if not run.check("切到靜音播放器後 12 秒內沒有新字幕", not new, json.dumps(new, ensure_ascii=False)[:300]):
            return False
        screenshot(run, "2-silent")

        # 3) 切回 TTS:應再出現(正向對照)
        last_id = max([y["id"] for y in after], default=0)
        if not await choose(tts_pid, "回 TTS"):
            return False
        hits3 = await wait_for(lambda: tts_hits(last_id), 30, 1.0)
        run.result["phase3"] = hits3
        if not run.check("切回 TTS 後再出現 TTS 字幕", bool(hits3), json.dumps(hits3, ensure_ascii=False)[:300]):
            return False
        screenshot(run, "3-back")

        await js("window.__v.btn('停止').click(), true")
        stopped = await wait_for(lambda: js("!!window.__v.btn('開始')"), 60)
        run.check("停止後回到可開始", bool(stopped))
        return all(c["ok"] for c in run.result["checks"]) and tts_alive
    finally:
        await ws.close()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data-dir", default=r"D:\codeproject\translate\data")
    ap.add_argument("--launch-timeout", type=int, default=600)
    a = ap.parse_args()

    evidence = SKILL / "evidence" / datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    evidence.mkdir(parents=True, exist_ok=True)
    run = Run(evidence)
    procs = {}
    ours: set = set()
    ok = False
    try:
        # Doctor:拒絕雙開
        busy = [p for p in (DEV_PORT, CDP_PORT) if port_in_use(p)]
        existing = image_pids("tingyi.exe")
        if busy or existing:
            run.check("沒有其他實例(拒絕雙開)", False, f"埠占用 {busy}、tingyi.exe {sorted(existing)}")
            return 1
        data = Path(a.data_dir)
        need = [data / "models" / "silero_vad.onnx",
                data / "models" / "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17" / "model.int8.onnx"]
        missing = [str(p) for p in need if not p.exists()]
        if not run.check("模型就緒", not missing, ", ".join(missing)):
            return 1

        env = dict(os.environ, TINGYI_DATA_DIR=str(data),
                   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={CDP_PORT}")
        app_log = open(evidence / "app.log", "w", encoding="utf-8", errors="replace")
        run.log("啟動 pnpm tauri dev …")
        procs["tauri dev"] = subprocess.Popen("pnpm tauri dev", shell=True, cwd=ROOT, env=env,
                                              stdout=app_log, stderr=subprocess.STDOUT, creationflags=NO_WINDOW)
        t0 = time.monotonic()
        while time.monotonic() - t0 < a.launch_timeout and find_page(CDP_PORT, URL_PREFIX) is None:
            if procs["tauri dev"].poll() is not None:
                break
            time.sleep(2)
        if not run.check("主視窗頁面出現在 CDP", find_page(CDP_PORT, URL_PREFIX) is not None,
                         f"{time.monotonic() - t0:.0f} s"):
            return 1
        ours = image_pids("tingyi.exe") - existing
        run.log(f"tingyi.exe pid:{sorted(ours)}")

        lines = evidence / "tts-lines.txt"
        lines.write_text("\n".join(TTS_LINES) + "\n", encoding="utf-8")
        silent = evidence / "silence.wav"
        write_silent_wav(silent)
        procs["TTS"] = subprocess.Popen(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                                         str(ROOT / "scripts" / "tts-smoke.ps1"), "-Lines", str(lines),
                                         "-NoWait", "-GapMs", "1200"],
                                        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                        creationflags=NO_WINDOW)
        procs["靜音播放器"] = subprocess.Popen(["powershell", "-NoProfile", "-Command",
                                           f"$p = New-Object System.Media.SoundPlayer '{silent}'; $p.PlayLooping(); Start-Sleep 900"],
                                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=NO_WINDOW)
        run.log(f"TTS pid {procs['TTS'].pid}、靜音播放器 pid {procs['靜音播放器'].pid}")
        time.sleep(3)
        ok = asyncio.run(drive(run, procs["TTS"].pid, procs["靜音播放器"].pid))
        return 0 if ok else 1
    except Exception as e:  # 驗證腳本本身出錯也要留下紀錄並判失敗
        run.check("驗證流程未拋例外", False, repr(e))
        return 1
    finally:
        for label in ("TTS", "靜音播放器", "tauri dev"):
            if label in procs:
                kill_tree(run, procs[label].pid, label)
        for pid in ours:  # taskkill /T 通常已帶走;仍存活才依 PID 補關,不按名稱殺
            kill_tree(run, pid, "tingyi.exe")
        (evidence / "result.json").write_text(json.dumps(run.result, ensure_ascii=False, indent=2), encoding="utf-8")
        code = 0 if ok else 1
        (evidence / "exit_code.txt").write_text(str(code), encoding="utf-8")
        run.log(f"結束,exit {code};證據 {evidence}")
        run.log_f.close()


if __name__ == "__main__":
    sys.exit(main())
