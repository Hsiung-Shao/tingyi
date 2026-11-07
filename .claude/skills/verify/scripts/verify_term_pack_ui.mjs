// 實跑驗證:在 app 內管理遊戲術語包——設定頁「遊戲術語」匯入、清單、查詢、使用這個(字幕頁同步)、移除。
// 匯入用的檔案由 scripts/poe-terms/build_term_pack.py 產生在證據目錄(代表使用者自己的資料夾);
// 原生檔案對話框無法自動操作,匯入步驟直接呼叫與按鈕相同的 import_term_pack 命令。需要本機 POE DB。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_term_pack_ui.mjs
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync, copyFileSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const argData = process.argv.indexOf('--data');
const SRC_DATA = argData > 0 ? process.argv[argData + 1] : 'D:\\codeproject\\translate\\data';
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-term-pack-ui`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
mkdirSync('D:\\tingyi-verify-tmp', { recursive: true });
const DATA = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'tpui-'));
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


const SV_DIR = 'sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17';
let devProc;
const startedPids = new Set();

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  mkdirSync(path.join(DATA, 'models', SV_DIR), { recursive: true });
  for (const f of ['model.int8.onnx', 'tokens.txt']) copyFileSync(path.join(SRC_DATA, 'models', SV_DIR, f), path.join(DATA, 'models', SV_DIR, f));
  copyFileSync(path.join(SRC_DATA, 'models', 'silero_vad.onnx'), path.join(DATA, 'models', 'silero_vad.onnx'));
  // 術語包先產生在「使用者自己的資料夾」(證據目錄),再由 app 匯入
  const userDir = path.join(OUT, 'my-terms');
  const gen = execFileSync('python', [path.join(ROOT, 'scripts', 'poe-terms', 'build_term_pack.py'), '--game', 'poe1', '--out', userDir], { encoding: 'utf8', env: { ...process.env, PYTHONIOENCODING: 'utf-8' } });
  log(gen.trim());
  const srcPack = path.join(userDir, 'poe1.json');

  log('啟動 pnpm tauri dev …');
  const appLog = createWriteStream(path.join(OUT, 'app.log'));
  devProc = spawn('pnpm tauri dev', { shell: true, cwd: ROOT, env: ENV, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  devProc.stdout.pipe(appLog);
  devProc.stderr.pipe(appLog);
  const appT = await waitFor(async () => {
    const t = (await cdpJson(APP_PORT, '/json/list', 1)).find((x) => x.type === 'page' && x.url.startsWith(`http://localhost:${DEV_PORT}/`) && !x.url.includes('overlay'));
    return { ok: !!t, t };
  }, 600_000, 2000);
  if (!check('主視窗頁面出現在 CDP', appT.ok)) return;
  for (const p of pids('tingyi.exe')) startedPids.add(p);
  const ui = connect(appT.t.webSocketDebuggerUrl);
  await ui.send('Page.enable');
  const js = (e) => ui.evalJs(e);
  const tab = (k) => js(`document.querySelector('[data-tab=${k}]').click(), true`);
  const nav = (k) => js(`document.querySelector('[data-nav=${k}]').click(), true`);
  await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-nav=terms]')`) }), 60_000);

  // 1. 一開始沒有術語包
  check('字幕頁沒有「遊戲術語」選單', !(await js(`!!document.querySelector('[data-role=term-pack]')`)));
  await tab('settings');
  await nav('terms');
  check('設定頁「遊戲術語」顯示尚未安裝', await js(`!!document.querySelector('[data-role=term-empty]')`));
  await screenshot(ui, '01-empty.png');

  // 2. 匯入(原生檔案對話框無法自動操作:直接呼叫匯入命令,與按鈕選檔後走的是同一個命令)
  const imp = await js(`window.__TAURI_INTERNALS__.invoke('import_term_pack', { path: ${JSON.stringify(srcPack)}, replace: false }).then((r) => ({ ok: true, r }), (e) => ({ ok: false, e: String(e) }))`);
  check('匯入 poe1.json 成功', imp.ok && imp.r.id === 'poe1', JSON.stringify(imp));
  const installed = path.join(DATA, 'terms', 'poe1.json');
  check('資料目錄出現 terms/poe1.json 且與原檔相同', existsSync(installed) && readFileSync(installed).equals(readFileSync(srcPack)));
  // 前端在按鈕流程裡會重讀清單;這裡直接呼叫命令,重新整理頁面讓清單更新
  await ui.send('Page.reload', {});
  await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-tab=settings]')`) }), 30_000);
  await tab('settings');
  await nav('terms');
  const card = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-pack-id=poe1]')`) }), 15_000);
  check('清單出現 Path of Exile 1 卡片', card.ok, await js(`document.querySelector('[data-pack-id=poe1]')?.innerText ?? ''`));
  const again = await js(`window.__TAURI_INTERNALS__.invoke('import_term_pack', { path: ${JSON.stringify(srcPack)}, replace: false }).then(() => 'ok', (e) => String(e))`);
  check('同名再匯入回 exists(前端據此詢問是否取代)', again === 'exists:poe1', again);
  const replaced = await js(`window.__TAURI_INTERNALS__.invoke('import_term_pack', { path: ${JSON.stringify(srcPack)}, replace: true }).then(() => 'ok', (e) => String(e))`);
  check('選擇取代後匯入成功', replaced === 'ok', replaced);
  const bad = path.join(OUT, 'not-a-pack.json');
  writeFileSync(bad, '{"hello": 1}');
  const badRes = await js(`window.__TAURI_INTERNALS__.invoke('import_term_pack', { path: ${JSON.stringify(bad)}, replace: false }).then(() => 'ok', (e) => String(e))`);
  check('匯入非術語包的 JSON 被拒', badRes !== 'ok', badRes);

  // 3. 查詢
  const setInput = async (sel, v) =>
    js(`(() => { const i = document.querySelector('${sel}'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(i, ${JSON.stringify(v)}); i.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
  await setInput('[data-role=term-query]', 'chaos orb');
  const r1 = await waitFor(async () => {
    const t = await js(`document.querySelector('[data-term-en="Chaos Orb"]')?.innerText ?? ''`);
    return { ok: t.includes('混沌石'), t };
  }, 10_000, 300);
  check('查「chaos orb」看到 Chaos Orb → 混沌石', r1.ok, r1.t);
  await screenshot(ui, '02-search-en.png');
  await setInput('[data-role=term-query]', '奇塔');
  const r2 = await waitFor(async () => {
    const t = await js(`document.querySelector('[data-term-en="Kitava"]')?.innerText ?? ''`);
    return { ok: t.includes('奇塔弗'), t };
  }, 10_000, 300);
  check('查「奇塔」看到 Kitava → 奇塔弗', r2.ok, r2.t);

  // 4. 使用這個 → 字幕頁下拉同步
  await js(`document.querySelector('[data-pack-id=poe1] [data-role=term-use]').click(), true`);
  const act = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-pack-id=poe1] [data-role=term-active]')`) }), 5_000, 200);
  check('按「使用這個」後標「使用中」', act.ok);
  await screenshot(ui, '03-in-use.png');
  await tab('main');
  const subSel = await js(`document.querySelector('#term-pack')?.value ?? ''`);
  check('字幕頁「遊戲術語」下拉同步為 poe1', subSel === 'poe1', subSel);
  await screenshot(ui, '04-subtitle-select.png');

  // 5. 移除(先確認)
  await tab('settings');
  await nav('terms');
  await js(`document.querySelector('[data-pack-id=poe1] [data-role=term-remove]').click(), true`);
  await sleep(300);
  check('移除前先要求確認(檔案還在)', existsSync(installed));
  await js(`document.querySelector('[data-pack-id=poe1] [data-role=term-remove-confirm]').click(), true`);
  const gone = await waitFor(async () => ({ ok: !(await js(`!!document.querySelector('[data-pack-id=poe1]')`)) && (await js(`!!document.querySelector('[data-role=term-empty]')`)) }), 10_000, 300);
  check('確認移除後卡片消失、顯示尚未安裝', gone.ok);
  check('資料目錄的 terms/poe1.json 已刪除', !existsSync(installed));
  check('使用者原本的檔案不受影響', existsSync(srcPack));
  await tab('main');
  check('字幕頁「遊戲術語」選單消失', !(await js(`!!document.querySelector('[data-role=term-pack]')`)));
  await screenshot(ui, '05-removed.png');
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  for (const p of startedPids) killTree(p, 'tingyi.exe');
  killTree(devProc?.pid, 'pnpm tauri dev');
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
