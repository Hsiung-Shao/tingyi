// 實跑驗證:正式版(target/release/tingyi.exe,先跑 `pnpm tauri build --no-bundle`)的 OBS 直播網址。
// 正式版頁面資源從程式內嵌的 dist 取(asset_resolver),與開發版轉 vite 不同路徑:
// /history 回 obs.html、其 assets 腳本與 css 回 200 且型別正確、不存在的資產回 404(不退回 index.html),
// 無頭 Edge 載入 /history、/subtitle 並畫出頁面。空資料目錄,不開始回合。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_obs_release.mjs
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync, copyFileSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const argData = process.argv.indexOf('--data');
const SRC_DATA = argData > 0 ? process.argv[argData + 1] : 'D:\\codeproject\\translate\\data';
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-obs-release`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
mkdirSync('D:\\tingyi-verify-tmp', { recursive: true });
const DATA = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'obsrel-'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { checks: [], statuses: {} };
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
      const r = await fetch(`http://127.0.0.1:${port}${p}`);
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
    if (last.ok || last.stop) return { ...last, ok: !!last.ok, ms: Date.now() - t0 };
    await sleep(interval);
  }
  return { ...last, ok: false, ms: Date.now() - t0 };
}


const EXE = path.join(ROOT, 'target', 'release', 'tingyi.exe');
const EDGE = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const EDGE_PORT = 9341;
let appProc, edgeProc;

const rawGet = (port, target, host) =>
  new Promise((res) => {
    const s = net.connect(port, '127.0.0.1', () => s.write(`GET ${target} HTTP/1.1\r\nHost: ${host}\r\nConnection: close\r\n\r\n`));
    const chunks = [];
    s.on('data', (d) => chunks.push(d));
    s.on('end', () => {
      const buf = Buffer.concat(chunks).toString('utf8');
      const [head, ...rest] = buf.split('\r\n\r\n');
      res({ status: head.split('\r\n')[0], head, body: rest.join('\r\n\r\n') });
    });
    s.on('error', (e) => res({ status: `ERR ${e.message}`, head: '', body: '' }));
  });

async function main() {
  const busy = [];
  for (const p of [APP_PORT, EDGE_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  if (!check('release 執行檔存在', existsSync(EXE), `${EXE} ${existsSync(EXE) ? statSync(EXE).mtime.toISOString() : ''}`)) return;
  const appLog = createWriteStream(path.join(OUT, 'app.log'));
  appProc = spawn(EXE, [], { cwd: ROOT, env: ENV, stdio: ['ignore', 'pipe', 'pipe'] });
  appProc.stdout.pipe(appLog);
  appProc.stderr.pipe(appLog);
  const appT = await waitFor(async () => {
    const t = (await cdpJson(APP_PORT, '/json/list', 1)).find((x) => x.type === 'page' && x.url.startsWith('http://tauri.localhost') && !x.url.includes('overlay'));
    return { ok: !!t, t };
  }, 120_000, 1000);
  if (!check('release 主視窗出現在 CDP', appT.ok, appT.t?.url ?? '')) return;
  const ui = connect(appT.t.webSocketDebuggerUrl);
  await ui.send('Page.enable');
  const js = (e) => ui.evalJs(e);
  await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-tab=settings]')`) }), 30_000);
  await js(`document.querySelector('[data-tab=settings]').click(), true`);
  await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-role=obs-toggle]')`) }), 15_000);
  await js(`document.querySelector('[data-role=obs-toggle]').click(), true`);
  const urlR = await waitFor(async () => {
    const u = await js(`document.querySelector('[data-role=obs-url-history]')?.value ?? ''`);
    return { ok: /^http:\/\/127\.0\.0\.1:\d+\/history/.test(u), u };
  }, 15_000);
  if (!check('release:打開直播網址', urlR.ok, urlR.u)) return;
  const port = Number(new URL(urlR.u).port);
  const host = `127.0.0.1:${port}`;
  const page = await rawGet(port, '/history', host);
  const scripts = [...page.body.matchAll(/(?:src|href)="\/(assets\/[^"]+)"/g)].map((m) => m[1]);
  check('release /history 回 obs.html(含 assets 腳本)', page.status.includes('200') && /聽譯直播/.test(page.body) && scripts.length > 0, `${page.status} ${scripts.join(',')}`);
  for (const a of scripts) {
    const r = await rawGet(port, `/${a}`, host);
    const mime = (r.head.match(/Content-Type: ([^\r\n]+)/i) || [])[1] ?? '';
    check(`release /${a} 回 200 且型別正確`, r.status.includes('200') && (a.endsWith('.js') ? /javascript/.test(mime) : /css/.test(mime)) && r.body.length > 100, `${r.status} ${mime} ${r.body.length}B`);
  }
  const miss = await rawGet(port, '/assets/nope-123.js', host);
  check('release 不存在的資產回 404(不退回主視窗 index.html)', miss.status.includes('404'), miss.status);
  const edgeDir = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'edge-'));
  edgeProc = spawn(EDGE, ['--headless=new', `--remote-debugging-port=${EDGE_PORT}`, `--user-data-dir=${edgeDir}`, '--no-first-run', 'about:blank'], { stdio: 'ignore', windowsHide: true });
  const et = await waitFor(async () => {
    const t = (await cdpJson(EDGE_PORT, '/json/list', 1)).find((x) => x.type === 'page');
    return { ok: !!t, t };
  }, 30_000, 500);
  const p = connect(et.t.webSocketDebuggerUrl);
  await p.send('Page.enable');
  await p.send('Emulation.setDeviceMetricsOverride', { width: 900, height: 400, deviceScaleFactor: 1, mobile: false });
  await p.send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 40, g: 44, b: 52, a: 1 } });
  for (const v of ['history', 'subtitle']) {
    await p.send('Page.navigate', { url: `http://${host}/${v}` });
    const root = v === 'history' ? '[data-role=obs-history]' : '[data-role=overlay-content]';
    const ok = await waitFor(async () => ({ ok: await p.evalJs(`!!document.querySelector('${root}')`) }), 15_000);
    check(`release:無頭 Edge 載入 /${v} 並畫出頁面`, ok.ok);
    await screenshot(p, `0${v === 'history' ? 1 : 2}-release-${v}.png`);
  }
  await screenshot(ui, '03-release-settings.png');
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  killTree(edgeProc?.pid, 'msedge(headless)');
  killTree(appProc?.pid, 'tingyi.exe(release)');
  try {
    rmSync(DATA, { recursive: true, force: true });
  } catch {
    log(`暫存資料目錄刪除失敗:${DATA}`);
  }
  writeFileSync(path.join(OUT, 'result.json'), JSON.stringify(result, null, 2));
  const code = fails || !result.checks.length ? 1 : 0;
  writeFileSync(path.join(OUT, 'exit_code.txt'), String(code));
  log(`結束,exit ${code};證據 ${OUT}`);
  process.exit(code);
}
