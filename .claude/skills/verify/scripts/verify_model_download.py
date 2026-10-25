"""實跑驗證:app 內下載專用模型(日文 ReazonSpeech)。

流程(只在頁面內用 CDP 操作,不送 OS 鍵鼠輸入):
1. 臨時資料目錄:硬連結放 SenseVoice 與 VAD,沒有專用模型。
2. 本機假 Hugging Face:`/<repo>/resolve/<commit>/<file>` 先 307 轉到 `/cdn/...`,再以 Range/206 提供
   `D:\\codeproject\\translate\\data\\models` 裡的真實模型檔(SHA256 與規格表相同),限速讓取消來得及。
3. `pnpm tauri dev`(TINGYI_DATA_DIR、TINGYI_MODEL_BASE_URL 指向上面兩者)。
4. 專用模型引擎一開始不可選 → 下載日文 → 進度前進 → 取消(暫存檔保留)→ 繼續(伺服器收到 Range 續傳)
   → 已下載 → 引擎可選 → 選專用模型、鎖日文、開始 → app log 顯示載入了 reazonspeech → 停止。
證據寫到 evidence/<時間>/:run.log、exit_code.txt、app.log、server.log、截圖、result.json。

用法(專案根目錄):python .claude/skills/verify/scripts/verify_model_download.py
"""
import argparse
import asyncio
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cdp import connect, find_page  # noqa: E402
from verify_audio_source import (  # noqa: E402
    CDP_PORT, DEV_PORT, NO_WINDOW, ROOT, SKILL, URL_PREFIX, Run, image_pids, kill_tree, port_in_use, screenshot, wait_for,
)

RS_REPO = "reazon-research/reazonspeech-k2-v2"
RS_DIR = "sherpa-onnx-zipformer-ja-reazonspeech-2024-08-01"
PK_REPO = "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
PK_DIR = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
SV_DIR = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"

JS = r"""
window.__m = {
  row: (id) => document.querySelector(`[data-model-id="${id}"]`),
  btn: (id) => { const r = window.__m.row(id); return r ? [...r.querySelectorAll('button')][0] : null; },
  text: (id) => { const r = window.__m.row(id); return r ? r.textContent : ''; },
  ready: (id) => !!(window.__m.row(id) && window.__m.row(id).querySelector('[data-role=model-ready]')),
  engineSelect: () => [...document.querySelectorAll('select')].find(s => [...s.options].some(o => o.value === 'routed')),
  routedEnabled: () => { const s = window.__m.engineSelect(); const o = s && [...s.options].find(o => o.value === 'routed'); return !!o && !o.disabled; },
  chooseEngine: (v) => { const s = window.__m.engineSelect(); Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, v); s.dispatchEvent(new Event('change', { bubbles: true })); return s.value; },
  setChecked: (labelText, checked) => { const l = [...document.querySelectorAll('label')].find(x => x.textContent.includes(labelText)); const i = l && l.querySelector('input'); if (!i) return null; if (i.checked !== checked) i.click(); return i.checked; },
  btnText: (text) => [...document.querySelectorAll('button')].find(b => b.textContent.trim() === text) || null,
};
true
"""


def make_server(models_root: Path, log_f, bytes_per_sec: int):
    """假 Hugging Face:resolve 路徑 307 到 /cdn/,/cdn/ 依 Range 回 206/200,限速送出。"""
    repo_dirs = {RS_REPO: RS_DIR, PK_REPO: PK_DIR}
    requests = []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            pass

        def do_GET(self):  # noqa: N802
            rng = self.headers.get("Range")
            requests.append({"path": self.path, "range": rng})
            log_f.write(f"{datetime.datetime.now():%H:%M:%S.%f} GET {self.path} Range={rng}\n")
            log_f.flush()
            m = re.match(r"^/(.+)/resolve/([0-9a-f]+)/(.+)$", self.path)
            if m:
                self.send_response(307)
                self.send_header("Location", f"/cdn/{m.group(1)}/{m.group(3)}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            m = re.match(r"^/cdn/(.+)/([^/]+)$", self.path)
            path = models_root / repo_dirs.get(m.group(1), "_none") / m.group(2) if m else None
            if not path or not path.is_file():
                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            size = path.stat().st_size
            start = 0
            if rng and (mm := re.match(r"bytes=(\d+)-$", rng)):
                start = int(mm.group(1))
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{size - 1}/{size}")
            else:
                self.send_response(200)
            self.send_header("Content-Length", str(size - start))
            self.end_headers()
            chunk = 256 * 1024
            with open(path, "rb") as f:
                f.seek(start)
                try:
                    while data := f.read(chunk):
                        self.wfile.write(data)
                        time.sleep(len(data) / bytes_per_sec)
                except (ConnectionResetError, ConnectionAbortedError, BrokenPipeError):
                    pass  # 取消時用戶端會中斷連線

    class Server(ThreadingHTTPServer):
        daemon_threads = True

        def handle_error(self, request, client_address):
            # 取消後用戶端的閒置連線被關掉,讀下一個請求時會 ConnectionReset;不是錯誤
            if not isinstance(sys.exc_info()[1], (ConnectionResetError, ConnectionAbortedError)):
                super().handle_error(request, client_address)

    srv = Server(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, requests


async def drive(run: Run, data_dir: Path, requests: list) -> bool:
    ws, page = await connect(CDP_PORT, URL_PREFIX)
    try:
        async def js(expr, timeout=30.0):
            await page.eval(JS)
            return await page.eval(expr, timeout)

        ready = await wait_for(lambda: js("!!window.__m.row('reazonspeech') && !!window.__m.btnText('開始')"), 180)
        if not run.check("主視窗出現專用模型下載卡", bool(ready)):
            return False
        run.check("下載前專用模型引擎不可選", not await js("window.__m.routedEnabled()"))

        await js("window.__m.btn('reazonspeech').click(), true")
        moving = await wait_for(lambda: js("/\\d+%/.test(window.__m.text('reazonspeech')) && window.__m.btn('reazonspeech').textContent.trim() === '取消'"), 30)
        if not run.check("按下載後出現進度與取消鈕", bool(moving), await js("window.__m.text('reazonspeech')")):
            return False
        await asyncio.sleep(2)
        await js("window.__m.btn('reazonspeech').click(), true")  # 取消
        paused = await wait_for(lambda: js("window.__m.btn('reazonspeech') && window.__m.btn('reazonspeech').textContent.startsWith('繼續')"), 15)
        part = data_dir / "models" / ".download" / RS_DIR / "encoder-epoch-99-avg-1.int8.onnx.part"
        part_len = part.stat().st_size if part.exists() else 0
        run.result["part_len_after_cancel"] = part_len
        row_text = await js("window.__m.text('reazonspeech')")
        if not run.check("取消後顯示「繼續」且暫存檔保留", bool(paused) and part_len > 0, f"{row_text};.part {part_len} bytes"):
            return False
        screenshot(run, "1-cancelled")

        n_before = len(requests)
        await js("window.__m.btn('reazonspeech').click(), true")  # 繼續
        done = await wait_for(lambda: js("window.__m.ready('reazonspeech')"), 180, 1.0)
        resumed = [r for r in requests[n_before:] if r["range"] and r["path"].startswith("/cdn/") and "encoder" in r["path"]]
        run.result["resume_requests"] = resumed
        run.check("繼續時以 Range 從暫存檔續傳", any(int(r["range"].split("=")[1].rstrip("-")) > 0 for r in resumed), json.dumps(resumed))
        if not run.check("下載完成顯示「已下載」", bool(done), await js("window.__m.text('reazonspeech')")):
            return False
        final = data_dir / "models" / RS_DIR
        files = sorted(p.name for p in final.iterdir()) if final.exists() else []
        run.check("正式目錄有四個模型檔", len(files) == 4, ", ".join(files))
        enabled = await wait_for(lambda: js("window.__m.routedEnabled()"), 15)
        if not run.check("下載後專用模型引擎可選", bool(enabled)):
            return False
        screenshot(run, "2-downloaded")

        translate_on = await js("window.__m.setChecked('啟用本機翻譯', false)")
        run.log(f"翻譯勾選狀態:{translate_on}")
        await js("window.__m.chooseEngine('routed')")
        await js("(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'ja'); b && b.click(); return true; })()")
        await js("window.__m.btnText('開始').click(), true")
        running = await wait_for(lambda: js("!!window.__m.btnText('停止')"), 120)
        if not run.check("專用模型引擎可以開始", bool(running)):
            return False
        screenshot(run, "3-running")
        stale = await js("document.body.innerText.includes('下次按「開始」時生效')")
        run.check("開始前就下完的模型不顯示「下次開始才生效」", not stale)
        await js("window.__m.btnText('停止').click(), true")
        await wait_for(lambda: js("!!window.__m.btnText('開始')"), 60)
        return all(c["ok"] for c in run.result["checks"])
    finally:
        await ws.close()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--source-data", default=r"D:\codeproject\translate\data")
    ap.add_argument("--rate-mbps", type=float, default=20.0, help="假伺服器限速(MB/s)")
    a = ap.parse_args()
    evidence = SKILL / "evidence" / datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    evidence.mkdir(parents=True, exist_ok=True)
    run = Run(evidence)
    src_models = Path(a.source_data) / "models"
    data_dir = evidence / "data"
    procs, ours, srv, ok = {}, set(), None, False
    try:
        busy = [p for p in (DEV_PORT, CDP_PORT) if port_in_use(p)]
        existing = image_pids("tingyi.exe")
        if busy or existing:
            run.check("沒有其他實例(拒絕雙開)", False, f"埠占用 {busy}、tingyi.exe {sorted(existing)}")
            return 1
        need = [src_models / "silero_vad.onnx", src_models / SV_DIR / "model.int8.onnx", src_models / SV_DIR / "tokens.txt",
                src_models / RS_DIR / "tokens.txt"]
        missing = [str(p) for p in need if not p.exists()]
        if not run.check("來源模型就緒", not missing, ", ".join(missing)):
            return 1
        # 臨時資料目錄:同一磁碟用硬連結,不複製數百 MB
        (data_dir / "models" / SV_DIR).mkdir(parents=True)
        os.link(src_models / "silero_vad.onnx", data_dir / "models" / "silero_vad.onnx")
        for n in ("model.int8.onnx", "tokens.txt"):
            os.link(src_models / SV_DIR / n, data_dir / "models" / SV_DIR / n)

        server_log = open(evidence / "server.log", "w", encoding="utf-8")
        srv, requests = make_server(src_models, server_log, int(a.rate_mbps * 1_000_000))
        base = f"http://127.0.0.1:{srv.server_address[1]}"
        run.log(f"假 Hugging Face:{base}(限速 {a.rate_mbps} MB/s);資料目錄 {data_dir}")

        env = dict(os.environ, TINGYI_DATA_DIR=str(data_dir), TINGYI_MODEL_BASE_URL=base,
                   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={CDP_PORT}")
        app_log = open(evidence / "app.log", "w", encoding="utf-8", errors="replace")
        run.log("啟動 pnpm tauri dev …")
        procs["tauri dev"] = subprocess.Popen("pnpm tauri dev", shell=True, cwd=ROOT, env=env,
                                              stdout=app_log, stderr=subprocess.STDOUT, creationflags=NO_WINDOW)
        t0 = time.monotonic()
        while time.monotonic() - t0 < 600 and find_page(CDP_PORT, URL_PREFIX) is None:
            if procs["tauri dev"].poll() is not None:
                break
            time.sleep(2)
        if not run.check("主視窗頁面出現在 CDP", find_page(CDP_PORT, URL_PREFIX) is not None, f"{time.monotonic() - t0:.0f} s"):
            return 1
        ours = image_pids("tingyi.exe") - existing
        ok = asyncio.run(drive(run, data_dir, requests))
        app_log.flush()
        log_text = (evidence / "app.log").read_text(encoding="utf-8", errors="replace")
        loaded = re.search(r"模型載入完成[^\n]*", log_text)
        run.check("app log 顯示載入了日文專用模型", bool(loaded) and "reazonspeech" in loaded.group(0),
                  loaded.group(0) if loaded else "找不到「模型載入完成」")
        ok = ok and all(c["ok"] for c in run.result["checks"])
        return 0 if ok else 1
    except Exception as e:  # 驗證腳本本身出錯也要留下紀錄並判失敗
        run.check("驗證流程未拋例外", False, repr(e))
        ok = False
        return 1
    finally:
        if "tauri dev" in procs:
            kill_tree(run, procs["tauri dev"].pid, "tauri dev")
        for pid in ours:
            kill_tree(run, pid, "tingyi.exe")
        if srv:
            srv.shutdown()
        # 臨時資料目錄只留暫存清單,模型檔(硬連結與下載的檔案)刪掉不佔空間
        if data_dir.exists():
            listing = [str(p.relative_to(data_dir)) for p in data_dir.rglob("*") if p.is_file()]
            (evidence / "data-files.txt").write_text("\n".join(listing), encoding="utf-8")
            shutil.rmtree(data_dir, ignore_errors=True)
        (evidence / "result.json").write_text(json.dumps(run.result, ensure_ascii=False, indent=2), encoding="utf-8")
        code = 0 if ok else 1
        (evidence / "exit_code.txt").write_text(str(code), encoding="utf-8")
        run.log(f"結束,exit {code};證據 {evidence}")
        run.log_f.close()


if __name__ == "__main__":
    sys.exit(main())
