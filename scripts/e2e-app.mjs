// 實跑驗證:本機程式(atx-app.exe)+ 擴充自動連線。
// 用法:node scripts/e2e-app.mjs --out <證據目錄> --exe <atx-app.exe> --data <資料目錄> [--bridge] [--download-timeout-min 60]
// 1. 啟動本機程式(WebView2 開遠端偵錯),在程式介面按「下載」,等模型執行中;驗 /health 與 api key
// 2. Chromium 載入擴充,腳本不寫任何端點設定;驗擴充自動取得端點、fixture 頁翻譯成功
// 3. 強制結束本機程式:llama-server 跟著結束;再翻新段落時擴充自動喚起程式、恢復翻譯
// 4. (--bridge)在程式介面開啟 CLI agent;popup 切到 bridge 後用 codex 翻譯
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, existsSync, readdirSync } from 'node:fs';
import { tmpdir, homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const { values: opt } = parseArgs({
  options: {
    out: { type: 'string' },
    exe: { type: 'string' },
    data: { type: 'string' },
    bridge: { type: 'boolean', default: false },
    'download-timeout-min': { type: 'string', default: '60' },
  },
});
if (!opt.out || !opt.exe || !opt.data) throw new Error('--out、--exe、--data 必填');
mkdirSync(opt.out, { recursive: true });
mkdirSync(opt.data, { recursive: true });

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const EXT = path.join(ROOT, 'extension', 'dist');
const EXT_ID = 'jamiahhkhimbjajeddmncinemlmfkfhn';
const CHROME_PORT = 9341;
const APP_PORT = 9343;
const RUNTIME = path.join(opt.data, 'runtime.json');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let fails = 0;
const log = (...a) => console.log(...a);
const check = (name, cond, detail = '') => {
  log(`${cond ? 'PASS' : 'FAIL'} ${name}${detail ? '  ' + detail : ''}`);
  if (!cond) fails++;
};
const readRuntime = () => {
  try {
    return JSON.parse(readFileSync(RUNTIME, 'utf8'));
  } catch {
    return null;
  }
};
// 資料目錄經環境變數傳給本機程式;Chromium 也繼承,它啟動的 native host 因此讀同一份
const ENV = { ...process.env, ATX_DATA_DIR: opt.data, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${APP_PORT}` };

async function cdpJson(port, p) {
  for (let i = 0; i < 100; i++) {
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
  writeFileSync(path.join(opt.out, name), Buffer.from(r.result.data, 'base64'));
  log(`screenshot ${name}`);
}

async function waitFor(fn, timeoutMs, interval = 1000) {
  const t0 = Date.now();
  let last;
  while (Date.now() - t0 < timeoutMs) {
    last = await fn();
    if (last.ok) return { ...last, ms: Date.now() - t0 };
    await sleep(interval);
  }
  return { ...last, ok: false, ms: Date.now() - t0 };
}

function findChromium() {
  const base = path.join(process.env.LOCALAPPDATA ?? path.join(homedir(), 'AppData/Local'), 'ms-playwright');
  for (const d of readdirSync(base).filter((d) => /^chromium-\d+$/.test(d)).sort().reverse()) {
    const exe = path.join(base, d, 'chrome-win64', 'chrome.exe');
    if (existsSync(exe)) return exe;
  }
  throw new Error('找不到 ms-playwright Chromium');
}

const alive = (pid) => {
  try {
    return execFileSync('tasklist', ['/FI', `PID eq ${pid}`, '/NH'], { encoding: 'utf8' }).includes(String(pid));
  } catch {
    return false;
  }
};
const healthy = async (port) => fetch(`http://127.0.0.1:${port}/health`).then((r) => r.status === 200, () => false);

async function appPage() {
  const t = (await cdpJson(APP_PORT, '/json/list')).find((x) => x.type === 'page');
  const page = connect(t.webSocketDebuggerUrl);
  await page.send('Page.enable');
  return page;
}

const FIXTURE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Fixture: local app</title></head><body style="font:16px/1.6 Georgia,serif;max-width:720px;margin:40px auto">
<h1>Local translation app</h1>
<p id="p1">Machine translation is a sub-field of computational linguistics that investigates the use of software to translate text.</p>
<p id="p2">Running the model on your own computer keeps the page content private and works without an internet connection.</p>
<p id="p3">The browser extension finds the local app automatically, so users never need to type an address or a token.</p>
<div id="later"></div></body></html>`;

const fixture = createServer((_q, s) => {
  s.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
  s.end(FIXTURE);
}).listen(0, '127.0.0.1');
await new Promise((r) => fixture.once('listening', r));
const pageUrl = `http://127.0.0.1:${fixture.address().port}/`;

// 程式有單一實例:已有一份在跑(例如使用者自己開的)時,喚起會被轉給它,測試結果無效(error_e2e_single_instance_forwards_to_user_app)
const running = execFileSync('tasklist', ['/FI', 'IMAGENAME eq atx-app.exe', '/NH'], { encoding: 'utf8' });
if (running.includes('atx-app.exe')) {
  log('SKIP 已有 atx-app.exe 在執行,請先從系統匣結束再測\n' + running.trim());
  writeFileSync(path.join(opt.out, 'exit_code.txt'), '2');
  process.exit(2);
}

let app;
let chrome;
const startApp = () => {
  app = spawn(opt.exe, [], { env: ENV, stdio: 'ignore', detached: false });
  return app;
};

try {
  // ── 1. 本機程式:下載並啟動模型 ──
  startApp();
  const ui = await appPage();
  await sleep(1500);
  await screenshot(ui, '01-app-initial.png');
  const badge = () => ui.evalJs(`document.getElementById('model-badge').textContent`);
  const initial = await badge();
  log(`程式初始狀態:${initial}`);
  if (initial === '尚未下載') {
    check('未下載時顯示下載按鈕', await ui.evalJs(`!document.getElementById('btn-download').hidden`));
    await ui.evalJs(`document.getElementById('btn-download').click()`);
    let shotProgress = false;
    const dl = await waitFor(
      async () => {
        const b = await badge();
        if (!shotProgress && b === '下載中' && (await ui.evalJs(`!document.getElementById('progress').hidden`))) {
          await screenshot(ui, '02-app-downloading.png');
          shotProgress = true;
        }
        return { ok: b === '執行中' || b === '發生錯誤', b };
      },
      Number(opt['download-timeout-min']) * 60_000,
      3000,
    );
    check('按下載後下載、校驗並自動啟動模型', dl.b === '執行中', `${dl.b} ${Math.round(dl.ms / 1000)}s`);
  } else {
    const r = await waitFor(async () => ({ ok: (await badge()) === '執行中' }), 240_000);
    check('已下載時開啟程式就自動啟動模型', r.ok, `${Math.round(r.ms / 1000)}s`);
  }
  await screenshot(ui, '03-app-ready.png');
  const rt = readRuntime();
  check('runtime.json 記錄執行中的端點', rt?.status === 'ready' && rt.llama?.port > 0, JSON.stringify({ status: rt?.status, port: rt?.llama?.port }));
  check('llama-server /health 回 200', await healthy(rt.llama.port));
  // /health 與 /v1/models 在 llama-server 是公開端點,驗金鑰要打真正的推論端點
  const chat = (headers) =>
    fetch(`http://127.0.0.1:${rt.llama.port}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...headers },
      body: JSON.stringify({ messages: [{ role: 'user', content: 'hi' }], max_tokens: 1 }),
    }).then((r) => r.status, () => 0);
  const noKey = await chat({});
  const withKey = await chat({ Authorization: `Bearer ${rt.llama.apiKey}` });
  check('沒帶 api key 的請求被拒(其他網頁不能偷用)', noKey === 401 && withKey === 200, `noKey=${noKey} withKey=${withKey}`);
  check('程式介面顯示瀏覽器已登錄', await ui.evalJs(`[...document.querySelectorAll('#browsers li')].every((l)=>l.classList.contains('ok'))`));

  // ── 2. 擴充自動連線 ──
  const profile = mkdtempSync(path.join(tmpdir(), 'atx-e2e-app-'));
  chrome = spawn(
    findChromium(),
    ['--no-sandbox', `--user-data-dir=${profile}`, `--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, `--remote-debugging-port=${CHROME_PORT}`, '--no-first-run', '--window-size=1280,900', 'about:blank'],
    { env: ENV, stdio: 'ignore' },
  );
  let sw;
  for (let i = 0; i < 40 && !sw; i++) {
    sw = (await cdpJson(CHROME_PORT, '/json/list')).find((t) => t.type === 'service_worker' && t.url.startsWith(`chrome-extension://${EXT_ID}/`));
    if (!sw) await sleep(500);
  }
  const bg = connect(sw.webSocketDebuggerUrl);
  for (let i = 0; i < 40 && !(await bg.evalJs(`typeof chrome!=='undefined'&&!!chrome.storage`)); i++) await sleep(250);
  const settings = () => bg.evalJs(`chrome.storage.local.get('atx:settings').then((r)=>r['atx:settings']??null)`);
  const linked = await waitFor(async () => {
    const s = await settings();
    return { ok: !!s?.linked, s };
  }, 30_000);
  const s0 = linked.s;
  check(
    '擴充安裝後自動取得本機端點(腳本未寫設定)',
    linked.ok && s0.route === 'direct' && s0.direct.baseUrl === `http://127.0.0.1:${rt.llama.port}/v1` && s0.direct.apiKey === rt.llama.apiKey && s0.direct.format === 'single',
    JSON.stringify({ route: s0?.route, baseUrl: s0?.direct?.baseUrl, format: s0?.direct?.format }),
  );
  const t = await cdpJson(CHROME_PORT, `/json/new?${encodeURI(pageUrl)}`);
  const page = connect(t.webSocketDebuggerUrl);
  await page.send('Page.enable');
  await sleep(2000);
  const toggled = await bg.evalJs(`(async()=>{const [tab]=await chrome.tabs.query({url:${JSON.stringify(pageUrl + '*')}});return chrome.tabs.sendMessage(tab.id,{type:'toggle'})})()`);
  check('toggle 開啟翻譯', toggled?.enabled === true);
  const count = () => page.evalJs(`document.querySelectorAll('font.atx-target').length`);
  const tr = await waitFor(async () => ({ ok: (await count()) >= 3, n: await count() }), 120_000);
  check('fixture 段落經本機程式的模型翻好', tr.ok, `n=${tr.n} ${tr.ms}ms`);
  log(`  譯文:${await page.evalJs(`document.querySelector('#p1 font.atx-target')?.textContent`)}`);
  await screenshot(page, '04-page-translated.png');
  const popup = connect((await cdpJson(CHROME_PORT, `/json/new?chrome-extension://${EXT_ID}/popup/popup.html`)).webSocketDebuggerUrl);
  await popup.send('Page.enable');
  const pop = await waitFor(async () => ({ ok: /已連上本機程式/.test((await popup.evalJs(`document.getElementById('appLinkText')?.textContent??''`)) ?? ''), t: await popup.evalJs(`document.getElementById('appLinkText')?.textContent??''`) }), 15_000);
  check('popup 顯示已連上本機程式', pop.ok, pop.t);
  await screenshot(popup, '05-popup-linked.png');

  // ── 3. 強制結束本機程式後自動喚起 ──
  const oldPid = rt.pid;
  const oldPort = rt.llama.port;
  execFileSync('taskkill', ['/F', '/PID', String(oldPid)]);
  await sleep(1500);
  check('本機程式被強制結束後 llama-server 也跟著結束(Job Object)', !alive(oldPid) && !(await healthy(oldPort)), `pid ${oldPid} port ${oldPort}`);
  await page.evalJs(`(()=>{const p=document.createElement('p');p.id='p4';p.textContent='After the local app was closed, the extension started it again in the background and kept translating.';document.getElementById('later').appendChild(p)})()`);
  const re = await waitFor(async () => ({ ok: !!(await page.evalJs(`document.querySelector('#p4 font.atx-target')?.textContent`)), err: await page.evalJs(`document.getElementById('p4')?.dataset.atxState`) }), 240_000, 2000);
  const rt2 = readRuntime();
  check('翻新段落時擴充自動喚起本機程式並恢復翻譯', re.ok && rt2?.pid && rt2.pid !== oldPid, `state=${re.err} newPid=${rt2?.pid} ${Math.round(re.ms / 1000)}s`);
  log(`  譯文:${await page.evalJs(`document.querySelector('#p4 font.atx-target')?.textContent`)}`);
  const s1 = await settings();
  check('喚起後的新端點自動寫回設定', s1.direct.baseUrl === `http://127.0.0.1:${rt2?.llama?.port}/v1` && s1.direct.apiKey === rt2?.llama?.apiKey);
  await screenshot(page, '06-page-relaunched.png');

  // ── 4. CLI agent bridge ──
  if (opt.bridge) {
    // 喚起的程式是背景模式;再開一次程式會由單一實例喚出視窗(並帶出 WebView2 偵錯埠)
    const ui2 = await appPage().catch(() => null);
    const uiNow = ui2 ?? ui;
    await uiNow.evalJs(`(()=>{const t=document.getElementById('bridge-toggle');if(!t.checked){t.checked=true;t.dispatchEvent(new Event('change'))}})()`);
    const br = await waitFor(async () => ({ ok: !!readRuntime()?.bridge }), 30_000);
    check('程式介面開啟 CLI agent 後 bridge 執行中', br.ok);
    await screenshot(uiNow, '07-app-bridge.png');
    await popup.send('Page.reload');
    await sleep(1500);
    await popup.evalJs(`(()=>{const r=document.getElementById('route');r.value='bridge';r.dispatchEvent(new Event('change'))})()`);
    await sleep(1000);
    const s2 = await settings();
    const rt3 = readRuntime();
    check('bridge 端點與 token 自動寫入設定', s2.route === 'bridge' && s2.bridge.url === rt3.bridge.url && s2.bridge.token === rt3.bridge.token);
    await page.evalJs(`(()=>{const p=document.createElement('p');p.id='p5';p.textContent='This sentence was translated by a command-line agent through the bridge.';document.getElementById('later').appendChild(p)})()`);
    const cb = await waitFor(async () => ({ ok: !!(await page.evalJs(`document.querySelector('#p5 font.atx-target')?.textContent`)) }), 300_000, 3000);
    check('走 bridge(codex)翻譯成功', cb.ok, `${Math.round(cb.ms / 1000)}s`);
    log(`  譯文:${await page.evalJs(`document.querySelector('#p5 font.atx-target')?.textContent`)}`);
    await screenshot(page, '08-page-bridge.png');
  }
} catch (e) {
  log(`FAIL 例外 ${e.stack ?? e}`);
  fails++;
} finally {
  chrome?.kill();
  const rt = readRuntime();
  if (rt?.pid) {
    try {
      execFileSync('taskkill', ['/F', '/PID', String(rt.pid)]);
    } catch {}
  }
  app?.kill();
  fixture.close();
  writeFileSync(path.join(opt.out, 'exit_code.txt'), fails ? '1' : '0');
  log(`RESULT ${fails ? 'FAIL' : 'PASS'}`);
  process.exit(fails ? 1 : 0);
}
