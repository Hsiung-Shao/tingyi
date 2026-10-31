// 實跑驗證:聽譯兼任網頁翻譯的本機程式(共用翻譯伺服器 + Chrome 擴充自動連線)。
// 改寫自 agent-translator 的 scripts/e2e-app.mjs。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_web_translate.mjs [--data D:\codeproject\translate\data]
// 1. 拒絕雙開(tingyi.exe 或 1420/9339 埠在用就失敗);建置擴充;`pnpm tauri dev` 啟動聽譯(開 native host 註冊)。
// 2. 主視窗勾本機翻譯、選 Hy-MT2 1.8B、按「開始」→ 字幕取得共用伺服器(此時網頁尚未取得)。
// 3. Chromium 載入擴充(腳本不寫端點設定)→ 擴充 hello 讓網頁取得同一個伺服器:只有一個 llama-server、
//    擴充拿到 slots [2,3]、沒帶 api key 被拒;fixture 頁翻好。
// 4. 按「停止」字幕 → 網頁仍是 ready、伺服器還在,新段落照樣翻好。
// 5. 強制結束聽譯 → llama-server 跟著結束(Job Object);再翻新段落時擴充自動喚起聽譯並恢復翻譯。
// 證據:evidence/<時間>-web/,run.log、exit_code.txt、截圖、result.json。只關閉自己啟動的程序(依 PID)。
import { spawn, execFileSync, execSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, existsSync, readdirSync, appendFileSync, createWriteStream } from 'node:fs';
import { tmpdir, homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import net from 'node:net';

const { values: opt } = parseArgs({ options: { data: { type: 'string', default: 'D:\\codeproject\\translate\\data' } } });
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-web`);
mkdirSync(OUT, { recursive: true });
const EXT = path.join(ROOT, 'web', 'extension', 'dist');
const EXT_ID = 'jamiahhkhimbjajeddmncinemlmfkfhn';
const APP_PORT = 9339;
const CHROME_PORT = 9341;
const DEV_PORT = 1420;
const RUNTIME = path.join(opt.data, 'runtime.json');
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
const readRuntime = () => {
  try {
    return JSON.parse(readFileSync(RUNTIME, 'utf8'));
  } catch {
    return null;
  }
};
// 資料目錄經環境變數傳給聽譯;Chromium 也繼承,它啟動的 native host 因此讀同一份。
// 開發版預設不寫 native host 註冊,這裡打開(TINGYI_REGISTER_HOST=1)。
const ENV = { ...process.env, TINGYI_DATA_DIR: opt.data, TINGYI_REGISTER_HOST: '1', WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${APP_PORT}` };

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
const healthy = async (port) => fetch(`http://127.0.0.1:${port}/health`).then((r) => r.status === 200, () => false);

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

function findChromium() {
  const base = path.join(process.env.LOCALAPPDATA ?? path.join(homedir(), 'AppData/Local'), 'ms-playwright');
  for (const d of readdirSync(base).filter((d) => /^chromium-\d+$/.test(d)).sort().reverse()) {
    const exe = path.join(base, d, 'chrome-win64', 'chrome.exe');
    if (existsSync(exe)) return exe;
  }
  throw new Error('找不到 ms-playwright Chromium');
}

const FIXTURE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Fixture: tingyi web translation</title></head><body style="font:16px/1.6 Georgia,serif;max-width:720px;margin:40px auto">
<h1>Local translation app</h1>
<p id="p1">Machine translation is a sub-field of computational linguistics that investigates the use of software to translate text.</p>
<p id="p2">Running the model on your own computer keeps the page content private and works without an internet connection.</p>
<p id="p3">The browser extension finds the local app automatically, so users never need to type an address or a token.</p>
<div id="later"></div></body></html>`;

// 主視窗頁面裡的小工具(只在頁面內操作 DOM,不送 OS 鍵鼠輸入)
const APP_JS = `window.__w = {
  btn: (t) => [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === t) || null,
  setChecked: (labelText, checked) => {
    const l = [...document.querySelectorAll('label')].find((x) => x.textContent.includes(labelText));
    const i = l && l.querySelector('input');
    if (!i) return null;
    if (i.checked !== checked) i.click();
    return i.checked;
  },
  chooseModel: (name) => {
    const s = [...document.querySelectorAll('select')].find((x) => [...x.options].some((o) => o.value === name));
    if (!s) return null;
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, name);
    s.dispatchEvent(new Event('change', { bubbles: true }));
    return s.value;
  },
}; true`;

let devProc;
let chrome;
let fixture;
const startedPids = new Set();

async function main() {
  // ── Doctor:拒絕雙開(單一實例會把喚起轉給使用者的程式,見 error_e2e_single_instance_forwards_to_user_app)──
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT, CHROME_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  const llamaBefore = pids('llama-server.exe');
  const need = [path.join(opt.data, 'bin', 'llama', 'llama-server.exe'), path.join(opt.data, 'models', 'Hy-MT2-1.8B-Q8_0.gguf')];
  if (!check('llama-server 與 Hy-MT2 1.8B 就緒', need.every(existsSync), need.filter((p) => !existsSync(p)).join(', '))) return;

  log('建置擴充 …');
  execSync('pnpm --dir web --filter @atx/extension build', { cwd: ROOT, stdio: 'ignore' });
  check('擴充建置完成', existsSync(path.join(EXT, 'manifest.json')));

  fixture = createServer((_q, s) => {
    s.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    s.end(FIXTURE);
  }).listen(0, '127.0.0.1');
  await new Promise((r) => fixture.once('listening', r));
  const pageUrl = `http://127.0.0.1:${fixture.address().port}/`;

  // ── 1. 啟動聽譯 ──
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
  const ui = connect(appT.t.webSocketDebuggerUrl);
  await ui.send('Page.enable');
  const js = async (expr) => (await ui.evalJs(APP_JS), ui.evalJs(expr));
  const ready = await waitFor(async () => ({ ok: await js(`(() => { const b = __w.btn('開始'); return !!b && !b.disabled; })()`) }), 180_000);
  if (!check('主視窗就緒(開始可按)', ready.ok)) return;
  for (const p of pids('tingyi.exe')) startedPids.add(p);
  const rt0 = readRuntime();
  check('程式啟動後寫出 runtime.json(網頁翻譯尚未啟動)', rt0?.status === 'stopped' && !rt0.llama, JSON.stringify({ status: rt0?.status }));

  // ── 2. 開始字幕(1.8B)──
  await js(`__w.setChecked('啟用本機翻譯', true)`);
  const chosen = await js(`__w.chooseModel('Hy-MT2-1.8B-Q8_0')`);
  check('選了 Hy-MT2 1.8B', chosen === 'Hy-MT2-1.8B-Q8_0', String(chosen));
  await js(`__w.btn('開始').click(), true`);
  const running = await waitFor(async () => ({ ok: await js(`!!__w.btn('停止')`) }), 240_000);
  if (!check('按開始後進入執行中(字幕取得共用伺服器)', running.ok, `${Math.round(running.ms / 1000)}s`)) return;
  const llamaSub = pids('llama-server.exe').filter((p) => !llamaBefore.includes(p));
  check('字幕啟動了一個 llama-server', llamaSub.length === 1, llamaSub.join(','));
  await screenshot(ui, '01-app-subtitles-running.png');

  // ── 3. 擴充自動連線並共用同一個伺服器 ──
  const profile = mkdtempSync(path.join(tmpdir(), 'tingyi-web-e2e-'));
  chrome = spawn(
    findChromium(),
    ['--no-sandbox', `--user-data-dir=${profile}`, `--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, `--remote-debugging-port=${CHROME_PORT}`, '--no-first-run', '--window-size=1280,900', 'about:blank'],
    { env: ENV, stdio: 'ignore' },
  );
  let sw;
  for (let i = 0; i < 60 && !sw; i++) {
    sw = (await cdpJson(CHROME_PORT, '/json/list')).find((t) => t.type === 'service_worker' && t.url.startsWith(`chrome-extension://${EXT_ID}/`));
    if (!sw) await sleep(500);
  }
  if (!check('擴充 service worker 啟動', !!sw)) return;
  const bg = connect(sw.webSocketDebuggerUrl);
  for (let i = 0; i < 40 && !(await bg.evalJs(`typeof chrome!=='undefined'&&!!chrome.storage`)); i++) await sleep(250);
  const settings = () => bg.evalJs(`chrome.storage.local.get('atx:settings').then((r)=>r['atx:settings']??null)`);
  const linked = await waitFor(async () => {
    const s = await settings();
    return { ok: !!s?.linked && !!s?.direct?.baseUrl?.includes('127.0.0.1'), s };
  }, 60_000);
  const rt = readRuntime();
  const s0 = linked.s ?? {};
  check(
    '擴充自動取得聽譯的端點與 slots(腳本未寫設定)',
    linked.ok && rt?.status === 'ready' && s0.direct.baseUrl === `http://127.0.0.1:${rt.llama.port}/v1` && s0.direct.apiKey === rt.llama.apiKey && JSON.stringify(s0.direct.slots) === '[2,3]',
    JSON.stringify({ status: rt?.status, baseUrl: s0.direct?.baseUrl, slots: s0.direct?.slots, model: rt?.llama?.model }),
  );
  const llamaShared = pids('llama-server.exe').filter((p) => !llamaBefore.includes(p));
  check('網頁與字幕共用同一個 llama-server(沒有多開)', llamaShared.length === 1 && llamaShared[0] === llamaSub[0], llamaShared.join(','));
  check('網頁沿用字幕正在用的 1.8B', rt?.llama?.model === 'Hy-MT2-1.8B-Q8_0', rt?.llama?.model);
  const chat = (headers) =>
    fetch(`http://127.0.0.1:${rt.llama.port}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...headers },
      body: JSON.stringify({ messages: [{ role: 'user', content: 'hi' }], max_tokens: 1 }),
    }).then((r) => r.status, () => 0);
  const noKey = await chat({});
  const withKey = await chat({ Authorization: `Bearer ${rt.llama.apiKey}` });
  check('沒帶 api key 的請求被拒(其他網頁不能偷用)', noKey === 401 && withKey === 200, `noKey=${noKey} withKey=${withKey}`);

  const t = await cdpJson(CHROME_PORT, `/json/new?${encodeURI(pageUrl)}`);
  const page = connect(t.webSocketDebuggerUrl);
  await page.send('Page.enable');
  await sleep(2000);
  const toggled = await bg.evalJs(`(async()=>{const [tab]=await chrome.tabs.query({url:${JSON.stringify(pageUrl + '*')}});return chrome.tabs.sendMessage(tab.id,{type:'toggle'})})()`);
  check('toggle 開啟翻譯', toggled?.enabled === true);
  const count = () => page.evalJs(`document.querySelectorAll('font.atx-target').length`);
  const tr = await waitFor(async () => ({ ok: (await count()) >= 3, n: await count() }), 120_000);
  check('fixture 段落經聽譯的共用伺服器翻好(字幕同時在跑)', tr.ok, `n=${tr.n} ${tr.ms}ms`);
  log(`  譯文:${await page.evalJs(`document.querySelector('#p1 font.atx-target')?.textContent`)}`);
  await screenshot(page, '02-page-translated.png');
  const popup = connect((await cdpJson(CHROME_PORT, `/json/new?chrome-extension://${EXT_ID}/popup/popup.html`)).webSocketDebuggerUrl);
  await popup.send('Page.enable');
  const pop = await waitFor(async () => {
    const tx = (await popup.evalJs(`document.getElementById('appLinkText')?.textContent??''`)) ?? '';
    return { ok: /已連上/.test(tx), tx };
  }, 15_000);
  check('popup 顯示已連上本機程式', pop.ok, pop.tx);
  await screenshot(popup, '03-popup-linked.png');

  // ── 4. 停止字幕:網頁翻譯不受影響 ──
  await js(`__w.btn('停止').click(), true`);
  const stopped = await waitFor(async () => ({ ok: await js(`!!__w.btn('開始')`) }), 60_000);
  check('按停止後回到可開始', stopped.ok);
  const rtS = readRuntime();
  check('停止字幕後網頁翻譯仍是 ready、伺服器還在', rtS?.status === 'ready' && (await healthy(rtS.llama.port)), JSON.stringify({ status: rtS?.status }));
  await page.evalJs(`(()=>{const p=document.createElement('p');p.id='p4';p.textContent='Stopping the live subtitles does not interrupt translation in the browser.';document.getElementById('later').appendChild(p)})()`);
  const tr4 = await waitFor(async () => ({ ok: !!(await page.evalJs(`document.querySelector('#p4 font.atx-target')?.textContent`)) }), 120_000, 1000);
  check('停止字幕後新段落照樣翻好', tr4.ok, `${Math.round(tr4.ms / 1000)}s`);
  await screenshot(ui, '04-app-stopped.png');

  // ── 5. 強制結束聽譯後由擴充喚起 ──
  const oldPid = rtS.pid;
  const oldPort = rtS.llama.port;
  try {
    execFileSync('taskkill', ['/F', '/PID', String(oldPid)]);
  } catch {
    // 程序已經結束也沒關係,下面的檢查會確認它與 llama-server 都不在了
  }
  await sleep(2000);
  check('聽譯被強制結束後 llama-server 也跟著結束(Job Object)', !alive(oldPid) && !(await healthy(oldPort)), `pid ${oldPid} port ${oldPort}`);
  await page.evalJs(`(()=>{const p=document.createElement('p');p.id='p5';p.textContent='After the app was closed, the extension started it again in the background and kept translating.';document.getElementById('later').appendChild(p)})()`);
  const re = await waitFor(async () => ({ ok: !!(await page.evalJs(`document.querySelector('#p5 font.atx-target')?.textContent`)), st: await page.evalJs(`document.getElementById('p5')?.dataset.atxState`) }), 300_000, 2000);
  const rt2 = readRuntime();
  if (rt2?.pid) startedPids.add(rt2.pid);
  check('翻新段落時擴充自動喚起聽譯並恢復翻譯', re.ok && rt2?.pid && rt2.pid !== oldPid, `state=${re.st} newPid=${rt2?.pid} model=${rt2?.llama?.model} ${Math.round(re.ms / 1000)}s`);
  log(`  譯文:${await page.evalJs(`document.querySelector('#p5 font.atx-target')?.textContent`)}`);
  await screenshot(page, '05-page-relaunched.png');
  result.runtime = { first: rt, afterStop: rtS, relaunched: rt2 };
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  chrome?.kill();
  const rt = readRuntime();
  if (rt?.pid) startedPids.add(rt.pid);
  for (const p of startedPids) killTree(p, 'tingyi.exe');
  killTree(devProc?.pid, 'pnpm tauri dev');
  fixture?.close();
  writeFileSync(path.join(OUT, 'result.json'), JSON.stringify(result, null, 2));
  const code = fails || !result.checks.length ? 1 : 0;
  writeFileSync(path.join(OUT, 'exit_code.txt'), String(code));
  log(`結束,exit ${code};證據 ${OUT}`);
  process.exit(code);
}
