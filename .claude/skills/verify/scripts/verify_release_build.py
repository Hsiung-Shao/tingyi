"""實跑驗證:正式版(`pnpm tauri build`)的執行檔與 NSIS 安裝檔。

不安裝(不動使用者已安裝的版本),直接啟動 `target/release/tingyi.exe`(安裝檔內就是這個執行檔),
WebView2 開 CDP(正式版頁面是 `http://tauri.localhost/`):
1. 安裝檔存在、比執行檔新;執行檔是 GUI 子系統(正式版不開主控台)。
2. 主視窗載入、資料目錄的專用模型顯示「已下載」、專用模型引擎可選。
3. 選專用模型、鎖日文、開本機翻譯 → 開始 → 出現「停止」、llama-server 子程序啟動、
   app log 有「模型載入完成:routed[reazonspeech]」→ 停止。
4. 關閉後沒有殘留的 tingyi.exe / llama-server.exe(只關自己啟動的 PID)。
證據寫到 evidence/<時間>-release/:run.log、exit_code.txt、app.log、截圖、result.json。

用法(專案根目錄):python .claude/skills/verify/scripts/verify_release_build.py
介面設定不會存檔(前端無 localStorage、後端無設定檔),所以不會改到使用者平常的設定。
"""
import argparse
import asyncio
import datetime
import json
import os
import re
import struct
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cdp import connect, find_page  # noqa: E402
from verify_audio_source import CDP_PORT, ROOT, SKILL, Run, image_pids, kill_tree, port_in_use, screenshot, wait_for  # noqa: E402
from verify_model_download import JS  # noqa: E402

EXE = ROOT / "target" / "release" / "tingyi.exe"
NSIS = ROOT / "target" / "release" / "bundle" / "nsis"
URL_PREFIX = "http://tauri.localhost"


def pe_subsystem(path: Path) -> int:
    """PE 檔頭的 Subsystem(2 = GUI、3 = 主控台)。"""
    b = path.read_bytes()[:4096]
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    return struct.unpack_from("<H", b, pe + 24 + 68)[0]


async def drive(run: Run) -> bool:
    ws, page = await connect(CDP_PORT, URL_PREFIX)
    try:
        async def js(expr, timeout=30.0):
            await page.eval(JS)
            return await page.eval(expr, timeout)

        ready = await wait_for(lambda: js("!!window.__m.btnText('開始') && !!window.__m.row('reazonspeech')"), 120)
        if not run.check("正式版主視窗載入(開始鈕與專用模型卡)", bool(ready)):
            return False
        both = await wait_for(lambda: js("window.__m.ready('reazonspeech') && window.__m.ready('parakeet')"), 30)
        run.check("資料目錄的日文、英文專用模型顯示已下載", bool(both),
                  await js("window.__m.text('reazonspeech') + ' / ' + window.__m.text('parakeet')"))
        run.check("專用模型引擎可選", bool(await js("window.__m.routedEnabled()")))
        screenshot(run, "1-ready")

        before = image_pids("llama-server.exe")
        translate_on = await js("window.__m.setChecked('啟用本機翻譯', true)")
        run.log(f"翻譯勾選:{translate_on}")
        await js("window.__m.chooseEngine('routed')")
        await js("(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'ja'); b && b.click(); return true; })()")
        await js("window.__m.btnText('開始').click(), true")
        running = await wait_for(lambda: js("!!window.__m.btnText('停止')"), 180)
        if not run.check("專用模型引擎 + 本機翻譯可以開始", bool(running)):
            return False
        async def new_llama():
            return image_pids("llama-server.exe") - before

        llama = await wait_for(new_llama, 60)
        run.check("llama-server 子程序已啟動", bool(llama), str(sorted(llama or [])))
        run.result["llama_pids"] = sorted(llama or [])
        await asyncio.sleep(3)
        screenshot(run, "2-running")
        await js("window.__m.btnText('停止').click(), true")
        stopped = await wait_for(lambda: js("!!window.__m.btnText('開始')"), 60)
        run.check("停止後回到可開始狀態", bool(stopped))
        return all(c["ok"] for c in run.result["checks"])
    finally:
        await ws.close()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data-dir", help="覆寫 TINGYI_DATA_DIR(預設不設,正式版在本機會用專案內 data/)")
    a = ap.parse_args()
    evidence = SKILL / "evidence" / (datetime.datetime.now().strftime("%Y%m%d-%H%M%S") + "-release")
    evidence.mkdir(parents=True)
    run = Run(evidence)
    proc, ok = None, False
    try:
        existing = image_pids("tingyi.exe")
        if port_in_use(CDP_PORT) or existing:
            run.check("沒有其他實例(拒絕雙開)", False, f"CDP 埠占用 {port_in_use(CDP_PORT)}、tingyi.exe {sorted(existing)}")
            return 1
        setups = sorted(NSIS.glob("*_x64-setup.exe"), key=lambda p: p.stat().st_mtime)
        newest = setups[-1] if setups else None
        run.check("NSIS 安裝檔存在且比執行檔新", bool(newest) and EXE.exists() and newest.stat().st_mtime >= EXE.stat().st_mtime,
                  f"{newest.name if newest else '-'} {newest.stat().st_size if newest else 0} bytes")
        run.check("執行檔是 GUI 子系統(正式版)", pe_subsystem(EXE) == 2, f"subsystem {pe_subsystem(EXE)}")
        run.result["exe"] = {"path": str(EXE), "size": EXE.stat().st_size, "mtime": time.ctime(EXE.stat().st_mtime)}

        env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={CDP_PORT}")
        if a.data_dir:  # 破壞對照用:指向空目錄時「已下載」與開始都必須失敗
            env["TINGYI_DATA_DIR"] = a.data_dir
            run.log(f"TINGYI_DATA_DIR={a.data_dir}")
        app_log = open(evidence / "app.log", "w", encoding="utf-8", errors="replace")
        proc = subprocess.Popen([str(EXE)], cwd=str(EXE.parent), env=env, stdout=app_log, stderr=subprocess.STDOUT)
        run.log(f"啟動 {EXE} pid {proc.pid}")
        t0 = time.monotonic()
        while time.monotonic() - t0 < 60 and find_page(CDP_PORT, URL_PREFIX) is None:
            if proc.poll() is not None:
                break
            time.sleep(1)
        if not run.check("主視窗頁面出現在 CDP", find_page(CDP_PORT, URL_PREFIX) is not None, f"{time.monotonic() - t0:.0f} s"):
            return 1
        ok = asyncio.run(drive(run))
        app_log.flush()
        text = (evidence / "app.log").read_text(encoding="utf-8", errors="replace")
        loaded = re.search(r"模型載入完成[^\n]*", text)
        run.check("app log 顯示載入了日文專用模型", bool(loaded) and "reazonspeech" in loaded.group(0),
                  loaded.group(0) if loaded else "找不到「模型載入完成」")
        ok = ok and all(c["ok"] for c in run.result["checks"])
        return 0 if ok else 1
    except Exception as e:  # 驗證腳本本身出錯也要留下紀錄並判失敗
        run.check("驗證流程未拋例外", False, repr(e))
        ok = False
        return 1
    finally:
        if proc:
            kill_tree(run, proc.pid, "tingyi.exe(正式版)")
            time.sleep(1)
            left = image_pids("tingyi.exe")
            left_llama = set(run.result.get("llama_pids", [])) & image_pids("llama-server.exe")
            for pid in left_llama:  # 只關這次啟動的子程序
                kill_tree(run, pid, "llama-server.exe(殘留)")
            run.check("關閉後沒有殘留的 tingyi.exe / llama-server.exe", not left and not left_llama,
                      f"tingyi {sorted(left)}、llama-server {sorted(left_llama)}")
            ok = ok and not left and not left_llama
        (evidence / "result.json").write_text(json.dumps(run.result, ensure_ascii=False, indent=2), encoding="utf-8")
        code = 0 if ok else 1
        (evidence / "exit_code.txt").write_text(str(code), encoding="utf-8")
        run.log(f"結束,exit {code};證據 {evidence}")
        run.log_f.close()


if __name__ == "__main__":
    sys.exit(main())
