// 實跑驗證:自動更新。本機 HTTP 伺服器代替 GitHub Release,提供 scripts/make-update-manifest.py 產生的 latest.json
// 與簽章過的 0.1.99 測試安裝檔(先以 TAURI_SIGNING_PRIVATE_KEY 建置、版本暫改 0.1.99);開發版以
// TINGYI_UPDATE_ENDPOINT 指向它、TINGYI_UPDATE_DRY_RUN=1 只下載驗證不安裝(不能蓋掉使用者已安裝的聽譯)。
// 破壞對照:竄改 latest.json 的簽章後必須拒絕。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_updater.mjs
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync, copyFileSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const argData = process.argv.indexOf('--data');
const SRC_DATA = argData > 0 ? process.argv[argData + 1] : 'D:\\codeproject\\translate\\data';
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-updater`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
mkdirSync('D:\\tingyi-verify-tmp', { recursive: true });
const DATA = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'updater-'));
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


const NSIS = path.join(ROOT, 'target', 'release', 'bundle', 'nsis');
const VER = '0.1.99';
const SRV_PORT = 8765;
const SRV_DIR = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'update-srv-'));
let devProc, srvProc;
const startedPids = new Set();

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT, SRV_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  const setup = path.join(NSIS, `聽譯_${VER}_x64-setup.exe`);
  if (!check(`有簽章的 ${VER} 測試安裝檔(先以 TAURI_SIGNING_PRIVATE_KEY 建置)`, existsSync(setup) && existsSync(setup + '.sig'), setup)) return;

  // 1. 用發版腳本產生 latest.json,網址指向本機伺服器(代替 GitHub Release)
  const notes = path.join(SRV_DIR, 'notes.md');
  writeFileSync(notes, '測試更新說明:\n- 設定頁改版\n- OBS 直播網址');
  const gen = execFileSync('python', [path.join(ROOT, 'scripts', 'make-update-manifest.py'), '--version', VER, '--out-dir', SRV_DIR, '--notes-file', notes, '--base-url', `http://127.0.0.1:${SRV_PORT}`], { encoding: 'utf8' });
  log(gen.trim());
  const manifestPath = path.join(SRV_DIR, 'latest.json');
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  check('latest.json 指向本機安裝檔且帶簽章', manifest.platforms['windows-x86_64'].url.endsWith(`tingyi_${VER}_x64-setup.exe`) && manifest.platforms['windows-x86_64'].signature.length > 100, manifest.platforms['windows-x86_64'].url);
  srvProc = spawn('python', ['-m', 'http.server', String(SRV_PORT), '--bind', '127.0.0.1', '--directory', SRV_DIR], { stdio: 'ignore', windowsHide: true });
  await waitFor(async () => ({ ok: await portInUse(SRV_PORT) }), 10_000, 300);

  log('啟動 pnpm tauri dev(TINGYI_UPDATE_ENDPOINT 指向本機、TINGYI_UPDATE_DRY_RUN=1 不安裝)…');
  const appLog = createWriteStream(path.join(OUT, 'app.log'));
  devProc = spawn('pnpm tauri dev', {
    shell: true,
    cwd: ROOT,
    env: { ...ENV, TINGYI_UPDATE_ENDPOINT: `http://127.0.0.1:${SRV_PORT}/latest.json`, TINGYI_UPDATE_DRY_RUN: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
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
  // 上次跑可能按過「略過這版」:清掉,讓自動提示能出現
  await js(`(() => { try { localStorage.removeItem('tingyi.update.dismissed'); } catch {} return true; })()`);
  await ui.send('Page.reload', {});
  await sleep(1500);

  // 2. 開啟約 5 秒後自動檢查,標題列出現提示
  const hint = await waitFor(async () => ({ ok: (await js(`document.querySelector('[data-role=update-hint]')?.textContent ?? ''`)).includes(VER) }), 30_000, 500);
  check(`開啟後自動檢查:標題列出現「有新版 ${VER}」`, hint.ok, `${Math.round(hint.ms / 1000)}s`);
  await screenshot(ui, '01-header-hint.png');
  await js(`document.querySelector('[data-role=update-hint]').click(), true`);
  // DOM 元素傳不回 Node(會變 undefined):判斷要在頁面內做成布林
  const card = await waitFor(async () => ({ ok: await js(`(() => { const s = document.querySelector('[data-section=about]'); return !!s && !s.closest('.hidden') && !!document.querySelector('[data-role=update-card]'); })()`) }), 10_000, 300);
  check('按提示跳到設定頁「關於與更新」並顯示新版卡片', card.ok);
  const notesText = await js(`document.querySelector('[data-role=update-notes]')?.textContent ?? ''`);
  check('卡片顯示更新說明', notesText.includes('OBS 直播網址'), notesText.replace(/\n/g, ' / '));
  const cur = await js(`document.querySelector('[data-section=about]')?.textContent ?? ''`);
  check('顯示目前版本 0.1.1', cur.includes('聽譯 0.1.1'));
  await screenshot(ui, '02-about-found.png');

  // 3. 下載並更新(測試模式只驗證):應下載並通過簽章驗證
  await js(`document.querySelector('[data-role=update-install]').click(), true`);
  const ok = await waitFor(async () => {
    const err = await js(`document.querySelector('[data-role=update-error]')?.textContent ?? ''`);
    if (err) return { ok: false, err, stop: true };
    return { ok: await js(`!!document.querySelector('[data-role=update-verified]')`) };
  }, 120_000, 500);
  check('下載並通過簽章驗證(測試模式未安裝)', ok.ok, ok.err ?? `${Math.round(ok.ms / 1000)}s`);
  const logText = readFileSync(path.join(OUT, 'app.log'), 'utf8');
  check('app 紀錄有「下載完成並通過簽章驗證」', /更新 0\.1\.99 下載完成並通過簽章驗證/.test(logText));
  await screenshot(ui, '03-verified.png');

  // 4. 破壞對照:簽章被竄改 → 必須失敗
  const bad = JSON.parse(readFileSync(manifestPath, 'utf8'));
  const sig = Buffer.from(bad.platforms['windows-x86_64'].signature, 'base64').toString('utf8');
  // .sig 是 base64 包住 minisign 文字;改掉簽章那一行中間一個字元
  const lines = sig.split('\n');
  const i = lines.findIndex((l, n) => n > 0 && l.length > 40 && !l.startsWith('untrusted') && !l.startsWith('trusted'));
  lines[i] = lines[i].slice(0, 20) + (lines[i][20] === 'A' ? 'B' : 'A') + lines[i].slice(21);
  bad.platforms['windows-x86_64'].signature = Buffer.from(lines.join('\n'), 'utf8').toString('base64');
  writeFileSync(manifestPath, JSON.stringify(bad, null, 2));
  await js(`document.querySelector('[data-role=update-check]').click(), true`);
  await waitFor(async () => ({ ok: !!(await js(`document.querySelector('[data-role=update-install]') && !document.querySelector('[data-role=update-install]').disabled`)) }), 30_000, 300);
  await js(`document.querySelector('[data-role=update-install]').click(), true`);
  const rejected = await waitFor(async () => {
    const err = await js(`document.querySelector('[data-role=update-error]')?.textContent ?? ''`);
    if (await js(`!!document.querySelector('[data-role=update-verified]')`)) return { ok: false, err: '竄改後仍顯示驗證通過', stop: true };
    return { ok: !!err, err };
  }, 120_000, 500);
  check('簽章被竄改時拒絕並顯示錯誤', rejected.ok, rejected.err);
  await screenshot(ui, '04-bad-signature.png');
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  for (const p of startedPids) killTree(p, 'tingyi.exe');
  killTree(devProc?.pid, 'pnpm tauri dev');
  killTree(srvProc?.pid, 'python http.server');
  try {
    rmSync(DATA, { recursive: true, force: true });
    rmSync(SRV_DIR, { recursive: true, force: true });
  } catch {
    log('暫存目錄刪除失敗(可能仍被佔用)');
  }
  writeFileSync(path.join(OUT, 'result.json'), JSON.stringify(result, null, 2));
  const code = fails || !result.checks.length ? 1 : 0;
  writeFileSync(path.join(OUT, 'exit_code.txt'), String(code));
  log(`結束,exit ${code};證據 ${OUT}`);
  process.exit(code);
}
