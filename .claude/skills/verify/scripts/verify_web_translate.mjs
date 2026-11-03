// 實跑驗證:聽譯兼任網頁翻譯的本機程式(共用翻譯伺服器 + Chrome 擴充自動連線)。
// 改寫自 agent-translator 的 scripts/e2e-app.mjs。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_web_translate.mjs [--data D:\codeproject\translate\data]
// 1. 拒絕雙開(tingyi.exe 或 1420/9339 埠在用就失敗);建置擴充;`pnpm tauri dev` 啟動聽譯(開 native host 註冊)。
//    資料目錄用暫存目錄(bin、models 以 junction 指到 --data),不碰安裝版的 settings/runtime/native-host。
// 2. 網頁翻譯卡片是「未啟動」、網頁模型預設 1.8B;字幕選 7B 按「開始」,開始選項存進 settings.json。
// 3. Chromium 載入擴充 → 網頁取得同一個伺服器,字幕優先所以暫用 7B,卡片寫明;slots [2,3]、沒帶 key 被拒;fixture 翻好。
// 4. 停止字幕 → 伺服器切回 1.8B(port/key 不變),卡片更新。5. M6:每段翻譯 p50 ≤ 1.5 s。
// 6. 網站規則 auto → 新分頁自動翻譯;設定頁列出網站。7. 卡片停止/啟動。
// 8. 強制結束聽譯 → llama-server 跟著結束;擴充喚起聽譯,用網頁的 1.8B。
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
const WEB_MODEL = 'Hy-MT2-1.8B-Q8_0';
const SUB_MODEL = 'Hy-MT2-7B-Q4_K_M';
// 聽譯用的資料目錄:暫存目錄,bin 與 models 以 junction 指到真的資料目錄
const DATA = mkdtempSync(path.join(tmpdir(), 'tingyi-web-data-'));
const RUNTIME = path.join(DATA, 'runtime.json');
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
const ENV = { ...process.env, TINGYI_DATA_DIR: DATA, TINGYI_REGISTER_HOST: '1', WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${APP_PORT}` };

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
  const need = [path.join(opt.data, 'bin', 'llama', 'llama-server.exe'), path.join(opt.data, 'models', WEB_MODEL + '.gguf'), path.join(opt.data, 'models', SUB_MODEL + '.gguf')];
  if (!check('llama-server、Hy-MT2 1.8B 與 7B 就緒', need.every(existsSync), need.filter((p) => !existsSync(p)).join(', '))) return;
  // 隔離的資料目錄:bin、models 用 junction 指到真的資料目錄,settings/runtime/native-host 不碰安裝版的那份
  for (const d of ['bin', 'models']) execFileSync('cmd', ['/c', 'mklink', '/J', path.join(DATA, d), path.join(opt.data, d)], { stdio: 'ignore' });
  check('隔離資料目錄建立(沒有 settings.json → 預設值)', existsSync(path.join(DATA, 'models')) && !existsSync(path.join(DATA, 'settings.json')), DATA);

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
  const ws0 = await js(`window.__TAURI_INTERNALS__.invoke('web_status')`);
  const card0 = await js(`document.querySelector('[data-role=web-status]')?.textContent ?? null`);
  check('主視窗網頁翻譯卡片:未啟動、網頁模型預設 1.8B', ws0?.web_model === WEB_MODEL && /未啟動/.test(card0 ?? ''), JSON.stringify({ web_model: ws0?.web_model, card: card0 }));
  await screenshot(ui, '01-app-web-card.png');

  // ── 2. 字幕用 7B 開始 ──
  await js(`__w.setChecked('啟用本機翻譯', true)`);
  const chosen = await js(`__w.chooseModel('${SUB_MODEL}')`);
  check('字幕選了 Hy-MT2 7B', chosen === SUB_MODEL, String(chosen));
  await js(`__w.btn('開始').click(), true`);
  const running = await waitFor(async () => ({ ok: await js(`!!__w.btn('停止')`) }), 300_000);
  if (!check('按開始後進入執行中(字幕取得共用伺服器)', running.ok, `${Math.round(running.ms / 1000)}s`)) return;
  const llamaSub = pids('llama-server.exe').filter((p) => !llamaBefore.includes(p));
  check('字幕啟動了一個 llama-server', llamaSub.length === 1, llamaSub.join(','));
  const saved = (() => {
    try {
      return JSON.parse(readFileSync(path.join(DATA, 'settings.json'), 'utf8'));
    } catch {
      return null;
    }
  })();
  check('主視窗開始選項存進 settings.json', saved?.prefs?.model === SUB_MODEL && saved?.web_model === WEB_MODEL, JSON.stringify(saved?.prefs ? { model: saved.prefs.model, web: saved.web_model } : saved));

  // ── 3. 擴充自動連線:網頁要 1.8B,字幕在用 7B → 暫用 7B,仍只有一個伺服器 ──
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
  const ws1 = await js(`window.__TAURI_INTERNALS__.invoke('web_status')`);
  const borrowed = await waitFor(async () => ({ ok: /暫用字幕的/.test((await js(`document.querySelector('[data-role=web-translate]')?.textContent ?? ''`)) ?? '') }), 10_000);
  check('字幕在用 7B 時網頁暫用 7B、卡片寫明會切回', rt?.llama?.model === SUB_MODEL && ws1?.server_model === SUB_MODEL && ws1?.subtitles && borrowed.ok, JSON.stringify(ws1));
  await screenshot(ui, '02-app-web-borrowing-7b.png');
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
  const tr = await waitFor(async () => ({ ok: (await count()) >= 3, n: await count() }), 180_000);
  check('fixture 段落翻好(字幕同時在跑、暫用 7B)', tr.ok, `n=${tr.n} ${tr.ms}ms`);
  log(`  譯文:${await page.evalJs(`document.querySelector('#p1 font.atx-target')?.textContent`)}`);
  await screenshot(page, '03-page-translated.png');

  // ── 4. 停止字幕:伺服器切回網頁的 1.8B,網頁照樣翻 ──
  await js(`__w.btn('停止').click(), true`);
  const stopped = await waitFor(async () => ({ ok: await js(`!!__w.btn('開始')`) }), 120_000);
  check('按停止後回到可開始', stopped.ok);
  const back = await waitFor(async () => {
    const r = readRuntime();
    return { ok: r?.status === 'ready' && r?.llama?.model === WEB_MODEL && (await healthy(r.llama.port)), r };
  }, 120_000);
  check('停止字幕後伺服器切回網頁的 1.8B(端點不變)', back.ok && back.r.llama.port === rt.llama.port && back.r.llama.apiKey === rt.llama.apiKey, JSON.stringify({ model: back.r?.llama?.model, port: back.r?.llama?.port, ms: back.ms }));
  const card1 = await waitFor(async () => {
    const tx = (await js(`document.querySelector('[data-role=web-translate]')?.textContent ?? ''`)) ?? '';
    return { ok: tx.includes(WEB_MODEL) && !/暫用字幕的/.test(tx), tx };
  }, 15_000);
  check('卡片顯示執行中 · 1.8B、不再寫暫用', card1.ok, (card1.tx ?? '').slice(0, 80));
  await screenshot(ui, '04-app-web-back-to-1.8b.png');

  // ── 5. M6:網頁翻譯速度(1.8B,每段單獨計時,文字不重複以免命中快取)──
  const times = [];
  for (let i = 0; i < 5; i++) {
    const id = `s${i}`;
    const text = `Speed sample ${i} at ${Date.now()}: the small local model should translate each paragraph in about a second.`;
    await page.evalJs(`(()=>{const p=document.createElement('p');p.id=${JSON.stringify(id)};p.textContent=${JSON.stringify(text)};document.getElementById('later').appendChild(p)})()`);
    const w = await waitFor(async () => ({ ok: !!(await page.evalJs(`document.querySelector('#${id} font.atx-target')?.textContent`)) }), 60_000, 100);
    times.push(w.ok ? w.ms : Infinity);
  }
  // 量到的時間含 content script 300 ms 的合併等待與輪詢間隔
  const p50 = [...times].sort((a, b) => a - b)[2];
  check('M6 網頁翻譯每段 p50 ≤ 1.5 s(1.8B)', p50 <= 1500, `p50=${p50}ms 各段=${times.join(',')}`);
  result.speed = times;

  // ── 6. 網站規則:新分頁開啟即自動翻譯,設定頁列出網站 ──
  await bg.evalJs(`(async()=>{const k='atx:settings';const s=(await chrome.storage.local.get(k))[k];s.sites={'127.0.0.1':{auto:true}};await chrome.storage.local.set({[k]:s});return true})()`);
  const t2 = await cdpJson(CHROME_PORT, `/json/new?${encodeURI(pageUrl + '?auto')}`);
  const page2 = connect(t2.webSocketDebuggerUrl);
  await page2.send('Page.enable');
  await page2.send('Page.bringToFront');
  const auto = await waitFor(async () => ({ ok: (await page2.evalJs(`document.querySelectorAll('font.atx-target').length`)) >= 3 }), 60_000);
  check('網站規則開啟自動翻譯:新分頁不用按就翻好', auto.ok, `${auto.ms}ms`);
  await screenshot(page2, '05-auto-translated.png');
  const optT = await cdpJson(CHROME_PORT, `/json/new?chrome-extension://${EXT_ID}/options/options.html`);
  const opts = connect(optT.webSocketDebuggerUrl);
  await opts.send('Page.enable');
  const listed = await waitFor(async () => ({ ok: /127\.0\.0\.1/.test((await opts.evalJs(`document.getElementById('sites')?.textContent ?? ''`)) ?? '') }), 10_000);
  check('設定頁網站規則清單列出這個網站', listed.ok);
  await screenshot(opts, '06-options-sites.png');
  await fetch(`http://127.0.0.1:${CHROME_PORT}/json/close/${optT.id}`).catch(() => null);
  await fetch(`http://127.0.0.1:${CHROME_PORT}/json/close/${t2.id}`).catch(() => null);
  await page.send('Page.bringToFront');

  // ── 7. 卡片的停止 / 啟動 ──
  await js(`document.querySelector('[data-role=web-toggle]').click(), true`);
  const off = await waitFor(async () => ({ ok: readRuntime()?.status === 'stopped' && pids('llama-server.exe').filter((p) => !llamaBefore.includes(p)).length === 0 }), 30_000);
  check('卡片按停止:網頁翻譯停止、伺服器結束(字幕沒在用)', off.ok);
  await js(`document.querySelector('[data-role=web-toggle]').click(), true`);
  const on = await waitFor(async () => {
    const r = readRuntime();
    return { ok: r?.status === 'ready' && r?.llama?.model === WEB_MODEL, r };
  }, 120_000);
  check('卡片按啟動:用 1.8B 啟動', on.ok, JSON.stringify({ status: on.r?.status, model: on.r?.llama?.model }));
  await screenshot(ui, '07-app-web-restarted.png');

  // ── 8. 強制結束聽譯後由擴充喚起:用網頁的 1.8B ──
  const rtS = readRuntime();
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
  check('翻新段落時擴充自動喚起聽譯、用網頁的 1.8B', re.ok && rt2?.pid && rt2.pid !== oldPid && rt2?.llama?.model === WEB_MODEL, `state=${re.st} newPid=${rt2?.pid} model=${rt2?.llama?.model} ${Math.round(re.ms / 1000)}s`);
  await screenshot(page, '08-page-relaunched.png');
  result.runtime = { first: rt, back: back.r, relaunched: rt2 };
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
  // junction 只移除連結本身(rmdir 不加 /s 不會刪到目標內容)
  for (const d of ['bin', 'models']) {
    try {
      execFileSync('cmd', ['/c', 'rmdir', path.join(DATA, d)], { stdio: 'ignore' });
    } catch {
      // 沒建立成功就沒有連結可移除
    }
  }
  writeFileSync(path.join(OUT, 'result.json'), JSON.stringify(result, null, 2));
  const code = fails || !result.checks.length ? 1 : 0;
  writeFileSync(path.join(OUT, 'exit_code.txt'), String(code));
  log(`結束,exit ${code};證據 ${OUT}`);
  process.exit(code);
}
