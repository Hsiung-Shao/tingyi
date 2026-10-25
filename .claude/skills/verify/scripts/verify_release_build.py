"""實跑驗證:正式版(`pnpm tauri build`)的執行檔與 NSIS 安裝檔。

不安裝(不動使用者已安裝的版本),直接啟動 `target/release/tingyi.exe`(與安裝檔內的執行檔同一次建置;
tauri 會改寫打包類型標記,位元組不一定相同),WebView2 開 CDP(正式版頁面是 `http://tauri.localhost/`):
1. 安裝檔與執行檔修改時間相差 120 秒內(同一次建置);執行檔是 GUI 子系統(正式版不開主控台)。
2. 主視窗載入、資料目錄的專用模型顯示「已下載」、專用模型引擎可選。
3. 選專用模型、鎖日文(找不到 ja 鈕判 FAIL)、開本機翻譯 → 開始 → 出現「停止」、
   llama-server 子程序啟動(PID 取自 app log `llama-server 啟動 pid=N`)、
   app log 有「模型載入完成:routed[reazonspeech](」→ 停止 → llama-server 由 app 結束。
4. 溫和關閉主視窗(taskkill 不加 /F,送 WM_CLOSE),app 30 秒內自行結束;沒結束才強制補殺並判 FAIL。
   之後只看自己的 tingyi PID 與 llama-server PID 有無殘留(使用者另有同名安裝版,不能用名稱判斷)。
沒帶 --data-dir 且專案 data/ 不存在時直接判 FAIL(正式版會退回使用者的 %LOCALAPPDATA%\\tingyi)。
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
from verify_audio_source import CDP_PORT, ROOT, SKILL, Run, image_pids, kill_tree, pid_alive, port_in_use, screenshot, wait_for  # noqa: E402
from verify_model_download import JS  # noqa: E402

EXE = ROOT / "target" / "release" / "tingyi.exe"
NSIS = ROOT / "target" / "release" / "bundle" / "nsis"
URL_PREFIX = "http://tauri.localhost"


def pe_subsystem(path: Path) -> int:
    """PE 檔頭的 Subsystem(2 = GUI、3 = 主控台)。"""
    b = path.read_bytes()[:4096]
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    return struct.unpack_from("<H", b, pe + 24 + 68)[0]


def log_llama_pids(log_path: Path) -> list:
    """app log 裡 `llama-server 啟動 pid=N` 的 PID(只認 app 自己啟動的)。"""
    text = log_path.read_text(encoding="utf-8", errors="replace")
    return sorted({int(m) for m in re.findall(r"llama-server 啟動 pid=(\d+)", text)})


async def drive(run: Run, app_log) -> bool:
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
        # 引擎選項依主視窗的 get_info,比模型卡的狀態晚到,要等
        enabled = await wait_for(lambda: js("window.__m.routedEnabled()"), 15)
        run.check("專用模型引擎可選", bool(enabled))
        screenshot(run, "1-ready")

        translate_on = await js("window.__m.setChecked('啟用本機翻譯', true)")
        run.log(f"翻譯勾選:{translate_on}")
        await js("window.__m.chooseEngine('routed')")
        ja = await js("(() => { const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'ja'); if (!b) return false; b.click(); return true; })()")
        run.check("鎖定日文(找到並點了 ja 鈕)", bool(ja))
        await js("window.__m.btnText('開始').click(), true")
        running = await wait_for(lambda: js("!!window.__m.btnText('停止')"), 180)
        if not run.check("專用模型引擎 + 本機翻譯可以開始", bool(running)):
            return False

        async def log_pids():
            app_log.flush()
            return log_llama_pids(app_log_path)

        app_log_path = Path(app_log.name)
        pids = await wait_for(log_pids, 60) or []
        run.result["llama_pids"] = pids
        run.check("llama-server 子程序已啟動(app log 有 pid 且存活)", bool(pids) and all(pid_alive(p) for p in pids), str(pids))
        await asyncio.sleep(3)
        screenshot(run, "2-running")
        await js("window.__m.btnText('停止').click(), true")
        stopped = await wait_for(lambda: js("!!window.__m.btnText('開始')"), 60)
        run.check("停止後回到可開始狀態", bool(stopped))

        async def llama_gone():
            return not any(pid_alive(p) for p in pids)

        gone = await wait_for(llama_gone, 15)
        run.check("停止後 llama-server 已由 app 結束", bool(pids) and bool(gone), str(pids))
        return all(c["ok"] for c in run.result["checks"])
    finally:
        await ws.close()


def execute(run: Run, a, evidence: Path, state: dict) -> bool:
    """啟動正式版並驅動;提早結束回 False(收尾由 main 的 finally 統一處理)。"""
    data_dir = ROOT / "data"
    if not a.data_dir and not data_dir.is_dir():
        return run.check("資料目錄存在(正式版預設用專案 data/)", False,
                         f"{data_dir} 不存在,正式版會退回使用者的 %LOCALAPPDATA%\\tingyi;請在 main 工作目錄跑或指定 --data-dir")
    existing = image_pids("tingyi.exe")
    if port_in_use(CDP_PORT) or existing:
        return run.check("沒有其他實例(拒絕雙開)", False, f"CDP 埠占用 {port_in_use(CDP_PORT)}、tingyi.exe {sorted(existing)}")
    setups = sorted(NSIS.glob("*_x64-setup.exe"), key=lambda p: p.stat().st_mtime)
    newest = setups[-1] if setups else None
    # tauri 在 makensis 前後各改寫一次執行檔的打包類型標記,執行檔會比安裝檔晚幾十毫秒;比同一次建置即可
    gap = abs(newest.stat().st_mtime - EXE.stat().st_mtime) if newest and EXE.exists() else None
    run.check("NSIS 安裝檔與執行檔是同一次建置", gap is not None and gap < 120,
              f"{newest.name if newest else '-'} {newest.stat().st_size if newest else 0} bytes;相差 {gap} s")
    run.check("執行檔是 GUI 子系統(正式版)", pe_subsystem(EXE) == 2, f"subsystem {pe_subsystem(EXE)}")
    run.result["exe"] = {"path": str(EXE), "size": EXE.stat().st_size, "mtime": time.ctime(EXE.stat().st_mtime)}

    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={CDP_PORT}")
    if a.data_dir:  # 破壞對照用:指向空目錄時「已下載」與開始都必須失敗
        env["TINGYI_DATA_DIR"] = a.data_dir
        run.log(f"TINGYI_DATA_DIR={a.data_dir}")
    app_log = state["app_log"] = open(evidence / "app.log", "w", encoding="utf-8", errors="replace")
    proc = state["proc"] = subprocess.Popen([str(EXE)], cwd=str(EXE.parent), env=env, stdout=app_log, stderr=subprocess.STDOUT)
    run.log(f"啟動 {EXE} pid {proc.pid}")
    t0 = time.monotonic()
    while time.monotonic() - t0 < 60 and find_page(CDP_PORT, URL_PREFIX) is None:
        if proc.poll() is not None:
            break
        time.sleep(1)
    if not run.check("主視窗頁面出現在 CDP", find_page(CDP_PORT, URL_PREFIX) is not None, f"{time.monotonic() - t0:.0f} s"):
        return False
    ok = asyncio.run(drive(run, app_log))
    app_log.flush()
    text = (evidence / "app.log").read_text(encoding="utf-8", errors="replace")
    # 自動語言會是 routed[reazonspeech+parakeet],所以要精確比對只載入日文專用模型
    loaded = re.search(r"模型載入完成:routed\[reazonspeech\]\([^\n]*", text)
    any_loaded = re.search(r"模型載入完成[^\n]*", text)
    run.check("app log 顯示只載入了日文專用模型(routed[reazonspeech])", bool(loaded),
              loaded.group(0) if loaded else (any_loaded.group(0) if any_loaded else "找不到「模型載入完成」"))
    return ok


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data-dir", help="覆寫 TINGYI_DATA_DIR(預設不設,正式版在本機會用專案內 data/)")
    a = ap.parse_args()
    evidence = SKILL / "evidence" / (datetime.datetime.now().strftime("%Y%m%d-%H%M%S") + "-release")
    evidence.mkdir(parents=True)
    run = Run(evidence)
    state = {"proc": None, "app_log": None}
    ok = False
    try:
        ok = execute(run, a, evidence, state)
    except Exception as e:  # 驗證腳本本身出錯也要留下紀錄並判失敗
        run.check("驗證流程未拋例外", False, repr(e))
        ok = False
    finally:
        proc, app_log = state["proc"], state["app_log"]
        if proc:
            # 溫和關閉:不加 /F,對視窗送 WM_CLOSE,app 要自己 shutdown 並結束
            if proc.poll() is None:
                subprocess.run(["taskkill", "/PID", str(proc.pid)], capture_output=True)
            try:
                proc.wait(timeout=30)
                exited = True
            except subprocess.TimeoutExpired:
                exited = False
            run.check("關閉主視窗後 app 自行結束", exited, f"pid {proc.pid}")
            if not exited:
                kill_tree(run, proc.pid, "tingyi.exe(正式版,強制補殺)")
            time.sleep(1)
            # 只看自己啟動的 PID(使用者電腦上有同名的安裝版,不能用名稱判斷)
            llama_pids = set(run.result.get("llama_pids", []))
            if app_log:
                app_log.flush()
                llama_pids |= set(log_llama_pids(Path(app_log.name)))
            left_app = pid_alive(proc.pid)
            left_llama = {p for p in llama_pids if pid_alive(p)}
            for pid in left_llama:
                kill_tree(run, pid, "llama-server.exe(殘留)")
            run.check("關閉後沒有殘留的 tingyi.exe / llama-server.exe", not left_app and not left_llama,
                      f"tingyi {proc.pid if left_app else '-'}、llama-server {sorted(left_llama)}")
        if app_log:
            app_log.close()
        ok = ok and all(c["ok"] for c in run.result["checks"])
        code = 0 if ok else 1
        (evidence / "result.json").write_text(json.dumps(run.result, ensure_ascii=False, indent=2), encoding="utf-8")
        (evidence / "exit_code.txt").write_text(str(code), encoding="utf-8")
        run.log(f"結束,exit {code};證據 {evidence}")
        run.log_f.close()
    return code


if __name__ == "__main__":
    sys.exit(main())
