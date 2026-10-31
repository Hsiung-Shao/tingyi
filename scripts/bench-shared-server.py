"""量測共用翻譯伺服器:網頁翻譯同時在跑時,字幕翻譯的首字延遲(M1)與顯示記憶體(M2)。

用與聽譯相同的參數單獨啟動一個 llama-server(-np 4、-c 12288、--api-key、--jinja),
字幕請求固定 id_slot 0、帶前文,串流量首字;網頁負載用 2 個執行緒持續送 id_slot 2 / 3 的段落(不串流,同擴充)。

用法(在專案根目錄):
  python scripts/bench-shared-server.py --data D:\\codeproject\\translate\\data --model Hy-MT2-1.8B-Q8_0 --rounds 40 --out bench-out/shared-server.json
"""
import argparse
import http.client
import json
import os
import secrets
import socket
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

LINES = [
    "The weather today is sunny with a light breeze from the west.",
    "Please remember to check the build settings before the next patch.",
    "Our team finished the final boss fight after three long attempts.",
    "The library will close early on Friday because of the holiday.",
    "She bought fresh bread and green apples at the morning market.",
    "Turn left at the second traffic light and the station is on your right.",
    "The new graphics card runs much cooler than the old one.",
    "We should upgrade the server before more players join tonight.",
]
PAGE = (
    "Rust is a general-purpose programming language emphasizing performance, type safety, and concurrency. "
    "It enforces memory safety, meaning that all references point to valid memory, without a garbage collector. "
    "To simultaneously enforce memory safety and prevent data races, its borrow checker tracks the object lifetime "
    "of all references in a program during compilation."
)
TARGET = "繁体中文(台湾用语)"


def subtitle_prompt(i: int) -> str:
    """與聽譯 `mt::build_prompt` 同樣格式:前 4 句的原文與譯文當背景,再翻第 i 句。"""
    ctx = "".join(f"{LINES[(i - k) % len(LINES)]} => 這是前文的譯文。\n" for k in range(4, 0, -1))
    return (
        "【背景信息】\n以下是同一段影片中前面几句的原文与译文,请保持称谓、语气与用词一致:\n"
        f"{ctx}\n请结合背景信息将以下文本翻译为{TARGET},这是影片字幕,请用台湾口语、简洁自然,"
        f"注意只需要输出翻译后的结果,不要额外解释。\n\n【待翻译文本】\n{LINES[i % len(LINES)]}"
    )


def web_prompt() -> str:
    return f"将以下文本翻译为{TARGET},注意只需要输出翻译后的结果,不要额外解释:\n\n{PAGE}"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def gpu_used_mib():
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits", "--id=0"],
            capture_output=True, text=True, timeout=10,
        ).stdout.strip()
        return int(out.splitlines()[0])
    except Exception:  # 沒有 NVIDIA 驅動時不量
        return None


def post(port, key, body, stream):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=120)
    conn.request("POST", "/v1/chat/completions", json.dumps(body),
                 {"Content-Type": "application/json", "Authorization": f"Bearer {key}"})
    return conn, conn.getresponse()


def first_token_ms(port, key, prompt) -> float:
    body = {"model": "hy-mt2", "messages": [{"role": "user", "content": prompt}], "stream": True,
            "id_slot": 0, "cache_prompt": True, "temperature": 0, "max_tokens": 200}
    t0 = time.perf_counter()
    conn, resp = post(port, key, body, True)
    if resp.status != 200:
        raise RuntimeError(f"字幕請求回 {resp.status}: {resp.read()[:200]!r}")
    first = None
    for raw in resp:
        line = raw.decode("utf-8", "replace").strip()
        if not line.startswith("data:") or line == "data: [DONE]":
            continue
        ev = json.loads(line[5:])
        delta = (ev.get("choices") or [{}])[0].get("delta", {}).get("content")
        if delta and first is None:
            first = (time.perf_counter() - t0) * 1000
    conn.close()
    return first if first is not None else float("nan")


class WebLoad:
    """2 個執行緒各自固定一個網頁 slot,持續送段落直到 stop。"""

    def __init__(self, port, key, slots=(2, 3)):
        self.port, self.key, self.slots = port, key, slots
        self.stop = threading.Event()
        self.latencies = []
        self.errors = 0
        self.threads = [threading.Thread(target=self.run, args=(s,), daemon=True) for s in slots]

    def run(self, slot):
        body = {"model": "hy-mt2", "messages": [{"role": "user", "content": web_prompt()}], "stream": False,
                "id_slot": slot, "cache_prompt": True, "temperature": 0, "max_tokens": 300}
        while not self.stop.is_set():
            t0 = time.perf_counter()
            try:
                conn, resp = post(self.port, self.key, body, False)
                resp.read()
                conn.close()
                if resp.status == 200:
                    self.latencies.append((time.perf_counter() - t0) * 1000)
                else:
                    self.errors += 1
            except Exception:
                self.errors += 1

    def __enter__(self):
        for t in self.threads:
            t.start()
        time.sleep(1.0)  # 讓兩個 slot 都在算
        return self

    def __exit__(self, *exc):
        self.stop.set()
        for t in self.threads:
            t.join(timeout=150)


def summary(xs):
    xs = sorted(x for x in xs if x == x)
    if not xs:
        return {"n": 0}
    p95 = xs[min(len(xs) - 1, int(round(0.95 * (len(xs) - 1))))]
    return {"n": len(xs), "p50": round(statistics.median(xs), 1), "p95": round(p95, 1), "max": round(xs[-1], 1)}


def measure(port, key, rounds):
    out = []
    for i in range(rounds):
        out.append(first_token_ms(port, key, subtitle_prompt(i)))
        time.sleep(0.3)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--model", default="Hy-MT2-1.8B-Q8_0")
    ap.add_argument("--rounds", type=int, default=40)
    ap.add_argument("--np", type=int, default=4, help="slot 數(共用伺服器為 4)")
    ap.add_argument("--ctx", type=int, default=12288, help="總 context(共用伺服器為 12288)")
    ap.add_argument("--no-load", action="store_true", help="只量沒有網頁負載的情況(slot 不足 4 個時用)")
    ap.add_argument("--out", default="bench-out/shared-server.json")
    a = ap.parse_args()
    data = Path(a.data)
    exe = data / "bin" / "llama" / "llama-server.exe"
    model = data / "models" / f"{a.model}.gguf"
    for p in (exe, model):
        if not p.is_file():
            print(f"找不到 {p}", file=sys.stderr)
            return 2
    port, key = free_port(), secrets.token_hex(32)
    before = gpu_used_mib()
    args = [str(exe), "-m", str(model), "-ngl", "all", "-c", str(a.ctx), "-np", str(a.np), "--host", "127.0.0.1",
            "--port", str(port), "--jinja", "--api-key", key]
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    log = open(Path(a.out).with_suffix(".llama.log"), "w", encoding="utf-8", errors="replace")
    proc = subprocess.Popen(args, cwd=exe.parent, stdout=log, stderr=subprocess.STDOUT, creationflags=0x08000000)
    try:
        t0 = time.time()
        while time.time() - t0 < 180:
            try:
                c = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                c.request("GET", "/health")
                if c.getresponse().status == 200:
                    break
            except OSError:
                pass
            if proc.poll() is not None:
                print("llama-server 提前結束", file=sys.stderr)
                return 1
            time.sleep(0.3)
        after = gpu_used_mib()
        measure(port, key, 4)  # 熱身:提示快取與 shader
        idle = measure(port, key, a.rounds)
        busy, web, errors = [], {"n": 0}, 0
        if not a.no_load:
            with WebLoad(port, key) as load:
                busy = measure(port, key, a.rounds)
            web, errors = summary(load.latencies), load.errors
        res = {
            "model": a.model,
            "args": f"-np {a.np} -c {a.ctx} --jinja --api-key",
            "vram_mib": {"before": before, "after_load": after,
                         "delta": (after - before) if before is not None and after is not None else None},
            "subtitle_first_token_ms": {"idle": summary(idle), "with_web_load": summary(busy)},
            "web_request_ms": web,
            "web_errors": errors,
        }
        print(json.dumps(res, ensure_ascii=False, indent=2))
        Path(a.out).write_text(json.dumps(res, ensure_ascii=False, indent=2), encoding="utf-8")
        return 0
    finally:
        proc.kill()
        proc.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
