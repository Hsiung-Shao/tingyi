// 實跑驗證:新電腦第一次使用(資料目錄全空)。「開始」不能按並說明缺什麼 → 在 app 內下載 SenseVoice 與 VAD →
// 關掉本機翻譯後就能開始,實際辨識管線用剛下載的模型啟動。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_fresh_install.mjs
// 會從 Hugging Face 與 GitHub 下載約 240 MB。資料目錄是暫存目錄,跑完刪除。只關閉自己啟動的程序(依 PID)。
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-fresh`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
const DATA = mkdtempSync(path.join(tmpdir(), 'tingyi-fresh-data-'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { checks: [] };
let fails = 0;
const log = (msg) => {
  const line = `${new Date().toTimeString().slice(0, 8)} ${msg}`;
  console.log(line);
  appendFileSync(path.join(OUT, 'run.log'), line + '\n');
};
const check = (name, ok, detail = '') => {
  result.checks.push({ name, ok: !!ok, detail });
  log(`[${ok ? 'PASS' : 'FAIL'}] ${name}${detail ? '  ' + detail : ''}`);
  if (!ok) fails++;
  return !!ok;
};
const ENV = { ...process.env, TINGYI_DATA_DIR: DATA, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${APP_PORT}` };

const portInUse = (port) =>
  new Promise((r) => {
    const s = net.connect(port, '127.0.0.1');
    s.once('connect', () => (s.destroy(), r(true)));
    s.once('error', () => r(false));
  });
const pids = (image) => {
  const out = execFileSync('tasklist', ['/FI', `IMAGENAME eq ${image}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8' });
  return [...out.matchAll(/^"[^"]+","(\d+)"/gm)].map((m) => Number(m[1]));
};
const alive = (pid) => {
  try {
    return execFileSync('tasklist', ['/FI', `PID eq ${pid}`, '/NH'], { encoding: 'utf8' }).includes(String(pid));
  } catch {
    return false;
  }
};
const killTree = (pid, label) => {
  if (!pid || !alive(pid)) return;
  try {
    execFileSync('taskkill', ['/PID', String(pid), '/T', '/F']);
  } catch {
    // 程序已經結束時 taskkill 回非 0,下一行的 alive() 會記錄實際結果
  }
  log(`關閉 ${label} pid ${pid}:${alive(pid) ? '仍存活!' : '已結束'}`);
};

async function cdpJson(port, p, tries = 100) {
  for (let i = 0; i < tries; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}${p}`, { method: p.startsWith('/json/new') ? 'PUT' : 'GET' });
      return await r.json();
    } catch {
      await sleep(300);
    }
  }
  throw new Error(`CDP ${port} 連不上`);
}

function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let id = 0;
  const pending = new Map();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    }
  };
  const ready = new Promise((r) => (ws.onopen = r));
  const send = async (method, params = {}) => {
    await ready;
    return new Promise((res) => {
      const i = ++id;
      pending.set(i, res);
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  };
  const evalJs = async (expr) => {
    const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400));
    return r.result?.result?.value;
  };
  return { send, evalJs, close: () => ws.close() };
}

async function screenshot(page, name) {
  const r = await page.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(path.join(OUT, name), Buffer.from(r.result.data, 'base64'));
  log(`截圖 ${name}`);
}

async function waitFor(fn, timeoutMs, interval = 1000) {
  const t0 = Date.now();
  let last = { ok: false };
  while (Date.now() - t0 < timeoutMs) {
    try {
      last = await fn();
    } catch (e) {
      last = { ok: false, err: String(e) };
    }
    if (last.ok) return { ...last, ms: Date.now() - t0 };
    await sleep(interval);
  }
  return { ...last, ok: false, ms: Date.now() - t0 };
}

/** 播放器字幕在 closed shadow root 裡,頁面讀不到:用 CDP 穿透 shadow root 讀譯文 */

const SV_DIR = 'sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17';
let devProc;
const startedPids = new Set();

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  check('資料目錄是空的(模擬新電腦)', !existsSync(path.join(DATA, 'models')), DATA);

  log('啟動 pnpm tauri dev …');
  const appLog = createWriteStream(path.join(OUT, 'app.log'));
  devProc = spawn('pnpm tauri dev', { shell: true, cwd: ROOT, env: ENV, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  devProc.stdout.pipe(appLog);
  devProc.stderr.pipe(appLog);
  const appT = await waitFor(async () => {
    const t = (await cdpJson(APP_PORT, '/json/list', 1)).find((x) => x.type === 'page' && x.url.startsWith(`http://localhost:${DEV_PORT}/`) && !x.url.includes('overlay'));
    return { ok: !!t, t };
  }, 600_000, 2000);
  if (!check('主視窗頁面出現在 CDP', appT.ok, `${Math.round(appT.ms / 1000)}s`)) return;
  for (const p of pids('tingyi.exe')) startedPids.add(p);
  const ui = connect(appT.t.webSocketDebuggerUrl);
  await ui.send('Page.enable');
  const js = (expr) => ui.evalJs(expr);
  const startBtn = `[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '開始')`;
  const hint = () => js(`document.querySelector('[data-role=start-missing]')?.textContent ?? ''`);

  // ── 1. 新電腦:開始不能按,而且說缺什麼 ──
  const blocked = await waitFor(async () => {
    const h = await hint();
    return { ok: (await js(`!!${startBtn} && ${startBtn}.disabled`)) && /SenseVoice/.test(h) && /VAD/.test(h), h };
  }, 60_000);
  check('全新資料目錄:開始不能按,提示缺 SenseVoice 與 VAD', blocked.ok, blocked.h);
  // 下載卡在「設定」分頁;左欄只有一行連結
  const goSet = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-role=go-settings]')`) }), 15_000);
  check('左欄提示到「設定」下載', goSet.ok);
  await js(`document.querySelector('[data-tab=settings]').click(), true`);
  await js(`document.querySelector('[data-nav=recog]').click(), true`);
  const cards = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-model-id=sensevoice]') && !!document.querySelector('[data-model-id=silero-vad]')`) }), 30_000);
  check('設定頁出現「辨識基本模型」下載卡', cards.ok);
  await screenshot(ui, '01-fresh-blocked.png');

  // ── 2. 在 app 內下載 SenseVoice 與 VAD ──
  for (const id of ['silero-vad', 'sensevoice']) {
    await js(`(() => { const b = [...document.querySelectorAll('[data-model-id=${id}] button')].find((x) => /下載|繼續/.test(x.textContent)); b && b.click(); return !!b; })()`);
  }
  const dl = await waitFor(async () => ({
    ok: await js(`['sensevoice','silero-vad'].every((id) => !!document.querySelector('[data-model-id=' + id + '] [data-role=model-ready]'))`),
  }), 900_000, 3000);
  check('在 app 內下載完 SenseVoice 與 VAD(卡片顯示已下載)', dl.ok, `${Math.round(dl.ms / 1000)}s`);
  const svFile = path.join(DATA, 'models', SV_DIR, 'model.int8.onnx');
  const vadFile = path.join(DATA, 'models', 'silero-vad', 'silero_vad.onnx');
  check('模型檔放在預期位置且大小正確', existsSync(svFile) && statSync(svFile).size === 239233841 && existsSync(vadFile) && statSync(vadFile).size === 643854, `${svFile} / ${vadFile}`);
  await screenshot(ui, '02-downloaded.png');
  await js(`document.querySelector('[data-tab=main]').click(), true`);

  // ── 3. 只剩翻譯伺服器沒下載;關掉本機翻譯後就能開始 ──
  const left = await waitFor(async () => {
    const h = await hint();
    return { ok: !/SenseVoice|VAD/.test(h) && /llama/.test(h), h };
  }, 30_000);
  check('下載後提示只剩翻譯伺服器', left.ok, left.h);
  await js(`(() => { const l = [...document.querySelectorAll('label')].find((x) => x.textContent.includes('啟用本機翻譯')); const i = l && l.querySelector('input'); if (i && i.checked) i.click(); return true; })()`);
  const can = await waitFor(async () => ({ ok: (await js(`!!${startBtn} && !${startBtn}.disabled`)) && (await hint()) === '' }), 30_000);
  check('關掉本機翻譯後「開始」可以按、提示消失', can.ok);
  await js(`${startBtn}.click(), true`);
  const running = await waitFor(async () => ({ ok: await js(`!![...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止')`) }), 120_000);
  check('按開始後用剛下載的模型進入執行中', running.ok, `${Math.round(running.ms / 1000)}s`);
  await screenshot(ui, '03-running.png');
  await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止').click(), true`);
  const stopped = await waitFor(async () => ({ ok: await js(`!!${startBtn}`) }), 60_000);
  check('按停止後回到可開始', stopped.ok);
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  for (const p of startedPids) killTree(p, 'tingyi.exe');
  killTree(devProc?.pid, 'pnpm tauri dev');
  // 暫存資料目錄是自己建立的(沒有 junction),整個刪掉
  try {
    rmSync(DATA, { recursive: true, force: true });
  } catch {
    log(`暫存資料目錄刪除失敗(可能仍被佔用):${DATA}`);
  }
  writeFileSync(path.join(OUT, 'result.json'), JSON.stringify(result, null, 2));
  const code = fails || !result.checks.length ? 1 : 0;
  writeFileSync(path.join(OUT, 'exit_code.txt'), String(code));
  log(`結束,exit ${code};證據 ${OUT}`);
  process.exit(code);
}
