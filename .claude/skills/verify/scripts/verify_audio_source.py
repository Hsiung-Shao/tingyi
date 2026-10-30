"""實跑驗證:指定程式音訊來源與執行中切換(聽譯 tingyi)。

流程(全程只在頁面內用 CDP 操作,不送 OS 鍵鼠輸入):
1. 拒絕雙開:1420 / 9339 埠或 tingyi.exe 已在跑就失敗。
2. `pnpm tauri dev`(WebView2 開遠端除錯埠),等主視窗「開始」可按。
3. 自己啟動兩個供音程式:TTS(powershell.exe 唸英文)與靜音播放器(powershell.exe 循環播放靜音 wav)。
4. 來源選 TTS → 開始 → 應出現與 TTS 句子相符的字幕;懸浮字幕有字。
5. 執行中切到靜音播放器 → 12 秒內不得新增字幕(TTS 仍在唸);懸浮字幕 10 秒內收起。
6. 切回 TTS → 應再出現相符字幕(正向對照,證明第 5 步不是因為 TTS 停了);懸浮字幕只顯示新句。
7. 單段上限輸入框;「沒聲音幾秒後隱藏」設 0 → 靜音 8 秒仍顯示 → 改回 3 → 3 秒內收起;
   收起時解鎖拖曳看得到字幕、鎖定後回到收起;填 100 輸入框顯示 30。
8. 停止、截圖、只關閉自己啟動的行程。證據寫到 evidence/<時間>/,run.log + exit_code.txt + 截圖 + result.json。

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
TTS_TITLE = "tingyi verify TTS"  # tts-window.ps1 的最小化視窗標題(不搶前景)

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
  optionsDetail: () => { const s = window.__v.select(); return s ? [...s.options].map(o => ({
    value: o.value, text: o.textContent, group: o.parentElement.tagName === 'OPTGROUP' ? o.parentElement.label : '' })) : []; },
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
  input: (label) => document.querySelector(`input[aria-label="${label}"]`),
  // 受控 input:原生 setter + input 事件更新 state,下一拍再送 Enter 觸發 onKeyDown 送出。
  // 不用 blur():視窗不在前景時 WebView2 不派送 focusout,React 的 onBlur 不會觸發。
  type: (label, v) => {
    const i = window.__v.input(label);
    if (!i) return null;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(i, v);
    i.dispatchEvent(new Event('input', { bubbles: true }));
    return i.value;
  },
  enter: (label) => {
    const i = window.__v.input(label);
    if (!i) return false;
    i.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    i.scrollIntoView({ block: 'center' });
    return true;
  },
  segLabel: () => ([...document.querySelectorAll('label')].find(l => l.textContent.startsWith('斷句')) || {}).textContent || '',
};
true
"""

MAX_LABEL = "單段上限(秒)"
HIDE_LABEL = "沒聲音幾秒後隱藏字幕"
# 懸浮字幕頁:內容容器的收起狀態、文字與顯示中的句子 id
OVERLAY_STATE = r"""(() => {
  const c = document.querySelector('[data-role=overlay-content]');
  if (!c) return null;
  return { idle: c.dataset.idle, text: c.innerText.trim(),
           ids: [...c.querySelectorAll('[data-sentence-id]')].map(e => +e.dataset.sentenceId) };
})()"""


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
    ows, opage = await connect(CDP_PORT, URL_PREFIX, overlay=True)
    try:
        async def js(expr, timeout=30.0):
            await page.eval(JS_HELPERS)  # 每次重注入:Vite HMR 重新載入會清掉
            return await page.eval(expr, timeout)

        async def sentences():
            return await js("window.__v.sentences()")

        async def overlay():
            return await opage.eval(OVERLAY_STATE)

        async def overlay_shown(min_id=0):
            o = await overlay()
            ok = o and o["idle"] == "0" and o["text"] and o["ids"] and min(o["ids"]) > min_id
            return o if ok else None

        async def watch_overlay(seconds: float) -> list:
            """每 0.25 s 記一次懸浮字幕狀態:[(距開始秒數, idle)]。"""
            t0 = time.monotonic()
            seen = []
            while time.monotonic() - t0 < seconds:
                o = await overlay()
                seen.append((round(time.monotonic() - t0, 2), o["idle"] if o else None))
                await asyncio.sleep(0.25)
            return seen

        ready = await wait_for(lambda: js("(() => { const b = window.__v.btn('開始'); return !!b && !b.disabled; })()"), 180)
        if not run.check("主視窗就緒(開始可按)", bool(ready)):
            return False
        engines = await js("""[...document.querySelectorAll('select option')]
            .filter(o => ['sensevoice', 'routed', 'whisper-turbo'].includes(o.value))
            .map(o => ({ value: o.value, enabled: !o.disabled, text: o.textContent }))""")
        routed = [e for e in engines if e["value"] == "routed"]
        run.check("辨識引擎下拉列出可選的「專用模型」", bool(routed) and routed[0]["enabled"], json.dumps(engines, ensure_ascii=False))
        translate_on = await js("window.__v.setChecked('啟用本機翻譯', false)")
        run.log(f"翻譯勾選狀態:{translate_on}(只驗擷取,不啟動 llama-server)")

        # TTS 程序有一個最小化的視窗:清單要像 OBS 那樣列出「[powershell.exe]: <標題>」,唸的時候在「播放中」
        tts_text = f"[powershell.exe]: {TTS_TITLE}"

        async def listed():
            await js("(() => { const b = document.querySelector('button[aria-label=\"重新整理程式清單\"]'); b && b.click(); return true; })()")
            await asyncio.sleep(1.5)
            opts = await js("window.__v.optionsDetail()")
            tts = [o for o in opts if o["text"] == tts_text and o["value"].startswith("win:")]
            silent = [o for o in opts if o["value"] == f"app:{silent_pid}"]
            return (tts[0], silent[0]) if tts and silent else None
        found = await wait_for(listed, 30, 0.5)
        if not run.check("清單以「[exe]: 視窗標題」列出 TTS 視窗,靜音播放器列在沒有視窗的程式",
                         bool(found), json.dumps(await js("window.__v.optionsDetail()"), ensure_ascii=False)[:600]):
            return False
        tts_value = found[0]["value"]

        async def tts_playing():
            await js("(() => { const b = document.querySelector('button[aria-label=\"重新整理程式清單\"]'); b && b.click(); return true; })()")
            await asyncio.sleep(1.5)
            opts = await js("window.__v.optionsDetail()")
            return any(o["text"] == tts_text and o["group"] == "播放中" for o in opts)
        run.check("TTS 視窗在「播放中」分組", bool(await wait_for(tts_playing, 20, 0.5)))

        async def choose(value: str, pid: int, label: str) -> bool:
            await js(f"window.__v.choose('{value}')")

            # 後端切好後,主視窗要再讀一次狀態才會更新下拉;兩者都到位才算(只等後端會差幾毫秒讀到舊值)
            async def switched():
                s = await js(f"window.__v.state().then(s => s.source.kind === 'app' && s.source.pid === {pid} ? s : null)")
                return s if s and await js("window.__v.select().value") == value else None
            st = await wait_for(switched, 5)
            shown = await js("window.__v.select().value")
            return run.check(f"來源切到{label}", bool(st) and shown == value, f"{json.dumps(st, ensure_ascii=False)};下拉顯示 {shown}")

        if not await choose(tts_value, tts_pid, " TTS 視窗"):
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
        ov1 = await wait_for(overlay_shown, 10)
        run.check("懸浮字幕有字(未收起)", bool(ov1), json.dumps(ov1 or await overlay(), ensure_ascii=False)[:300])
        screenshot(run, "1-tts")

        # 2) 執行中切到靜音播放器:不得新增字幕;懸浮字幕應在 10 秒內收起
        if not await choose(f"app:{silent_pid}", silent_pid, "靜音播放器"):
            return False
        t_switch = time.monotonic()
        seen = await watch_overlay(3)  # 切換前已在辨識/合併中的句子可能晚到
        before = await sentences()
        seen += [(round(t + 3, 2), i) for t, i in await watch_overlay(12)]
        after = await sentences()
        hidden_at = next((t for t, i in seen if i == "1"), None)
        run.result["phase2_overlay"] = seen
        run.check("切到靜音播放器後懸浮字幕 10 秒內收起", hidden_at is not None and hidden_at <= 10,
                  f"切換後 {hidden_at} s 收起(預期約 3.5–5 s),觀察 {time.monotonic() - t_switch:.1f} s")
        run.check("收起後到觀察結束都維持收起", hidden_at is not None and all(i == "1" for t, i in seen if t >= hidden_at),
                  json.dumps([x for x in seen if hidden_at is not None and x[0] >= hidden_at and x[1] != "1"]))
        new = [x for x in after if x["id"] > max([y["id"] for y in before], default=0)]
        run.result["phase2_new"] = new
        tts_alive = pid_alive(tts_pid)
        run.check("負向對照期間 TTS 仍在唸", tts_alive)
        if not run.check("切到靜音播放器後 12 秒內沒有新字幕", not new, json.dumps(new, ensure_ascii=False)[:300]):
            return False
        screenshot(run, "2-silent")

        # 3) 切回 TTS:應再出現(正向對照)
        last_id = max([y["id"] for y in after], default=0)
        if not await choose(tts_value, tts_pid, "回 TTS 視窗"):
            return False
        hits3 = await wait_for(lambda: tts_hits(last_id), 30, 1.0)
        run.result["phase3"] = hits3
        if not run.check("切回 TTS 後再出現 TTS 字幕", bool(hits3), json.dumps(hits3, ensure_ascii=False)[:300]):
            return False
        ov3 = await wait_for(lambda: overlay_shown(last_id), 15)
        run.check("切回 TTS 後懸浮字幕只顯示收起後的新句", bool(ov3),
                  f"收起時最新 id {last_id};{json.dumps(ov3 or await overlay(), ensure_ascii=False)[:300]}")
        screenshot(run, "3-back")

        async def set_input(label: str, v: str):
            await js(f"window.__v.type('{label}', '{v}')")
            await asyncio.sleep(0.3)
            await js(f"window.__v.enter('{label}')")

        # 4) 斷句「單段上限」輸入框:填 4 → 執行中立即生效(標題顯示上限 4 秒);清空 → 回到中檔預設 8 秒
        async def set_max(v: str) -> str:
            await set_input(MAX_LABEL, v)
            want = f"上限 {v or 8} 秒"
            got = await wait_for(lambda: js(f"window.__v.segLabel().includes('{want}') ? window.__v.segLabel() : null"), 10)
            return got or await js("window.__v.segLabel()")
        label4 = await set_max("4")
        run.check("單段上限填 4 秒後執行中生效", "上限 4 秒" in label4, label4)
        screenshot(run, "4-max-speech")
        label0 = await set_max("")
        run.check("單段上限清空後回到中檔預設 8 秒", "上限 8 秒" in label0, label0)

        # 5) 「沒聲音幾秒後隱藏」設 0:切到靜音 8 秒仍顯示;改回 3 → 3 秒內收起
        await set_input(HIDE_LABEL, "0")
        shown5 = await wait_for(overlay_shown, 15)
        run.check("隱藏秒數設 0 前懸浮字幕有字", bool(shown5), json.dumps(shown5 or await overlay(), ensure_ascii=False)[:300])
        if not await choose(f"app:{silent_pid}", silent_pid, "靜音播放器(隱藏秒數 0)"):
            return False
        seen0 = await watch_overlay(8)
        run.result["phase5_hide0"] = seen0
        run.check("隱藏秒數 0 時靜音 8 秒仍顯示", all(i == "0" for _, i in seen0), json.dumps([x for x in seen0 if x[1] != "0"]))
        screenshot(run, "5-hide-0")
        await set_input(HIDE_LABEL, "3")
        seen3 = await watch_overlay(3)
        run.result["phase5_hide3"] = seen3
        run.check("隱藏秒數改回 3 後 3 秒內收起", any(i == "1" for _, i in seen3), json.dumps(seen3))
        screenshot(run, "5-hide-3")

        # 6) 收起時解鎖拖曳:字幕要看得到(對位置用);鎖定後回到收起
        await js("window.__v.btn('解鎖以拖曳位置').click(), true")
        drag = await wait_for(lambda: overlay_shown(-1), 5)
        run.check("收起時解鎖拖曳,懸浮字幕看得到最後一句", bool(drag), json.dumps(drag or await overlay(), ensure_ascii=False)[:300])
        screenshot(run, "6-drag")
        await js("window.__v.btn('鎖定位置(恢復滑鼠穿透)').click(), true")
        relock = await wait_for(lambda: opage.eval(f"({OVERLAY_STATE}).idle === '1'"), 5)
        run.check("鎖定後懸浮字幕回到收起", bool(relock))

        # 7) 秒數超出範圍:輸入框改顯示實際生效的值
        await set_input(HIDE_LABEL, "100")
        clamped = await wait_for(lambda: js(f"window.__v.input('{HIDE_LABEL}').value === '30' || null"), 5)
        run.check("隱藏秒數填 100 後輸入框顯示 30", bool(clamped), await js(f"window.__v.input('{HIDE_LABEL}').value"))
        await set_input(HIDE_LABEL, "3")

        await js("window.__v.btn('停止').click(), true")
        stopped = await wait_for(lambda: js("!!window.__v.btn('開始')"), 60)
        run.check("停止後回到可開始", bool(stopped))
        return all(c["ok"] for c in run.result["checks"]) and tts_alive
    finally:
        await ws.close()
        await ows.close()


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
                                         str(SKILL / "scripts" / "tts-window.ps1"), "-Lines", str(lines),
                                         "-GapMs", "1200", "-Title", TTS_TITLE],
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
