"""最小的 Chrome DevTools Protocol 用戶端:連 WebView2 的遠端除錯埠,在頁面裡執行 JS。

只在頁面內操作 DOM,不送任何 OS 鍵鼠輸入(不會打進使用者的前景視窗)。
單獨使用:python cdp.py "<js 運算式>" [--port 9339] [--url-prefix http://localhost:1420/]
"""
import argparse
import asyncio
import json
import time
import urllib.request

import websockets


def list_targets(port: int) -> list:
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json", timeout=2) as r:
        return json.loads(r.read().decode("utf-8"))


def find_page(port: int, url_prefix: str, overlay: bool = False):
    """回傳第一個 url 以 url_prefix 開頭的 page target(overlay=True 找懸浮字幕頁,否則找主視窗);沒有回 None。"""
    try:
        targets = list_targets(port)
    except OSError:
        return None
    for t in targets:
        url = t.get("url", "")
        if t.get("type") == "page" and url.startswith(url_prefix) and ("overlay" in url) == overlay:
            return t
    return None


class Page:
    def __init__(self, ws):
        self.ws = ws
        self.next_id = 0

    async def eval(self, expr: str, timeout: float = 30.0):
        """執行 JS 運算式(可 await),回傳值;JS 拋例外時丟 RuntimeError。"""
        self.next_id += 1
        mid = self.next_id
        await self.ws.send(json.dumps({
            "id": mid,
            "method": "Runtime.evaluate",
            "params": {"expression": expr, "awaitPromise": True, "returnByValue": True},
        }))
        deadline = time.monotonic() + timeout
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError(f"CDP 逾時:{expr[:80]}")
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), left))
            if msg.get("id") != mid:
                continue  # 事件或其他回應
            res = msg.get("result", {})
            if "exceptionDetails" in res:
                raise RuntimeError(f"JS 例外:{res['exceptionDetails'].get('text')} {res.get('result', {}).get('description', '')}")
            return res.get("result", {}).get("value")


async def connect(port: int, url_prefix: str, overlay: bool = False) -> tuple:
    page = find_page(port, url_prefix, overlay)
    if page is None:
        raise RuntimeError(f"埠 {port} 上找不到 {url_prefix} 頁面")
    ws = await websockets.connect(page["webSocketDebuggerUrl"], max_size=None)
    return ws, Page(ws)


async def _main():
    ap = argparse.ArgumentParser()
    ap.add_argument("expr")
    ap.add_argument("--port", type=int, default=9339)
    ap.add_argument("--url-prefix", default="http://localhost:1420/")
    a = ap.parse_args()
    ws, page = await connect(a.port, a.url_prefix)
    try:
        print(json.dumps(await page.eval(a.expr), ensure_ascii=False, indent=2))
    finally:
        await ws.close()


if __name__ == "__main__":
    asyncio.run(_main())
