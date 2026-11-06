// 實跑驗證:「設定」分頁下載並選用翻譯模型、辨識引擎選用,以及 OBS 直播網址(/history、/subtitle、/events)。
// 資料目錄只放 SenseVoice、VAD、llama.cpp(從本機 data 複製);Hy-MT2 1.8B Q4(1.1 GB)實際從 Hugging Face 下載。
// OBS 瀏覽器來源用無頭 Edge(同為 Chromium)開兩頁並截圖;TTS 由 Windows 語音從預設輸出播放(會出聲)。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_settings_obs.mjs [--data <本機資料目錄>]
// 只關閉自己啟動的程序(依 PID)。
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync, copyFileSync, readFileSync, cpSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const argData = process.argv.indexOf('--data');
const SRC_DATA = argData > 0 ? process.argv[argData + 1] : 'D:\\codeproject\\translate\\data';
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-settings-obs`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
mkdirSync('D:\\tingyi-verify-tmp', { recursive: true });
const DATA = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'settings-'));
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
const Q4 = 'Hy-MT2-1.8B-Q4_K_M';
const EDGE = 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const EDGE_PORT = 9341;
let devProc, edgeProc, ttsProc;
const startedPids = new Set();

/** 原始 HTTP 請求(可自訂 Host),回狀態行。 */
const rawGet = (port, target, host) =>
  new Promise((res) => {
    const s = net.connect(port, '127.0.0.1', () => s.write(`GET ${target} HTTP/1.1\r\nHost: ${host}\r\nConnection: close\r\n\r\n`));
    let buf = '';
    s.on('data', (d) => {
      buf += d;
      if (buf.includes('\r\n')) (s.destroy(), res(buf.split('\r\n')[0]));
    });
    s.on('error', (e) => res(`ERR ${e.message}`));
    setTimeout(() => (s.destroy(), res(buf.split('\r\n')[0] || 'TIMEOUT')), 5000);
  });

/** 讀 SSE 一段時間,回收到的 data 行。 */
async function sseFor(url, ms) {
  const ac = new AbortController();
  const out = [];
  setTimeout(() => ac.abort(), ms);
  try {
    const r = await fetch(url, { signal: ac.signal });
    const rd = r.body.getReader();
    let buf = '';
    for (;;) {
      const { value, done } = await rd.read();
      if (done) break;
      buf += new TextDecoder().decode(value);
      let i;
      while ((i = buf.indexOf('\n\n')) >= 0) {
        const block = buf.slice(0, i);
        buf = buf.slice(i + 2);
        if (block.startsWith('data: ')) out.push(block.slice(6));
      }
    }
  } catch {
    // 時間到 abort
  }
  return out;
}

async function edgePage(url) {
  const r = await waitFor(async () => {
    const t = (await cdpJson(EDGE_PORT, '/json/list', 1)).find((x) => x.type === 'page');
    return { ok: !!t, t };
  }, 30_000, 500);
  const p = connect(r.t.webSocketDebuggerUrl);
  await p.send('Page.enable');
  await p.send('Emulation.setDeviceMetricsOverride', { width: 900, height: 600, deviceScaleFactor: 1, mobile: false });
  // OBS 底下是遊戲畫面;截圖改深灰底,透明背景上的字才看得到
  await p.send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 40, g: 44, b: 52, a: 1 } });
  await p.send('Page.navigate', { url });
  return p;
}

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT, EDGE_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  mkdirSync(path.join(DATA, 'models', SV_DIR), { recursive: true });
  for (const f of ['model.int8.onnx', 'tokens.txt']) copyFileSync(path.join(SRC_DATA, 'models', SV_DIR, f), path.join(DATA, 'models', SV_DIR, f));
  copyFileSync(path.join(SRC_DATA, 'models', 'silero_vad.onnx'), path.join(DATA, 'models', 'silero_vad.onnx'));
  cpSync(path.join(SRC_DATA, 'bin', 'llama'), path.join(DATA, 'bin', 'llama'), { recursive: true });
  check('暫存資料目錄沒有任何翻譯模型', !readdirSync(path.join(DATA, 'models'), { recursive: true }).some((f) => String(f).endsWith('.gguf')), DATA);

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
  const tab = (k) => js(`document.querySelector('[data-tab=${k}]').click(), true`);
  const mtSelect = `[...document.querySelectorAll('select')].find((s) => [...s.options].some((o) => o.value === '${Q4}'))`;
  const startBtn = `[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '開始')`;

  // ── 1. 設定分頁:全部模型都有下載卡 ──
  await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-tab=settings]')`) }), 60_000);
  const goSet = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-role=go-settings]')`) }), 20_000);
  check('字幕頁左欄提示翻譯模型要到設定下載', goSet.ok);
  await tab('settings');
  const rows = await waitFor(async () => {
    const ids = await js(`[...document.querySelectorAll('[data-section=translate] [data-model-id]')].map((x) => x.dataset.modelId)`);
    return { ok: ids.length === 11, ids };
  }, 30_000);
  check('設定頁翻譯區列出 llama.cpp + 10 個翻譯模型', rows.ok, JSON.stringify(rows.ids));
  const engines = await js(`[...document.querySelectorAll('[data-section=engines] [data-engine]')].map((x) => x.dataset.engine)`);
  check('設定頁列出 6 個辨識引擎', engines.length === 6, JSON.stringify(engines));
  const engCards = await waitFor(async () => ({
    ok: await js(`['whisper-turbo','qwen3-0.6b','qwen3-1.7b','cohere'].every((e) => document.querySelector('[data-engine="' + e + '"] [data-model-id]'))`),
  }), 15_000);
  check('需要下載的引擎各自有下載卡', engCards.ok);
  await screenshot(ui, '01-settings.png');

  // ── 2. 下載 1.8B Q4 並選用 ──
  await js(`(() => { const b = [...document.querySelectorAll('[data-model-id="hy-mt2-1.8b-q4"] button')].find((x) => /下載|繼續/.test(x.textContent)); b && b.click(); return !!b; })()`);
  const dl = await waitFor(async () => {
    const err = await js(`document.querySelector('[data-model-id="hy-mt2-1.8b-q4"] [data-role=model-error]')?.textContent ?? ''`);
    if (err) return { ok: false, err, stop: true };
    return { ok: await js(`!!document.querySelector('[data-model-id="hy-mt2-1.8b-q4"] [data-role=model-ready]')`) };
  }, 1_800_000, 3000);
  check('Hy-MT2 1.8B Q4:在設定頁下載完', dl.ok, `${Math.round(dl.ms / 1000)}s ${dl.err ?? ''}`);
  const q4File = path.join(DATA, 'models', 'hy-mt2-1.8b-q4', `${Q4}.gguf`);
  check('Q4 檔案在 models/hy-mt2-1.8b-q4/ 且大小正確', existsSync(q4File) && statSync(q4File).size === 1133080448, q4File);
  await js(`document.querySelector('[data-model-id="hy-mt2-1.8b-q4"] [data-role=model-use]').click(), true`);
  const active = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-model-id="hy-mt2-1.8b-q4"] [data-role=model-active]')`) }), 10_000);
  check('按「使用這個」後標「使用中」', active.ok);
  await js(`document.querySelector('[data-engine="sensevoice"] [data-role=engine-use]')?.click(), true`);
  const svActive = await waitFor(async () => ({ ok: await js(`!!document.querySelector('[data-engine="sensevoice"] [data-role=engine-active]')`) }), 5_000);
  check('辨識引擎 SenseVoice 標「使用中」', svActive.ok);
  await screenshot(ui, '02-settings-q4-active.png');
  await tab('main');
  const sel = await js(`${mtSelect}.value`);
  check('回字幕頁:翻譯模型下拉已是 Q4', sel === Q4, sel);
  const can = await waitFor(async () => ({ ok: await js(`!!${startBtn} && !${startBtn}.disabled`) }), 30_000);
  check('Q4 下載並選用後「開始」可以按', can.ok, await js(`document.querySelector('[data-role=start-missing]')?.textContent ?? ''`));

  // ── 3. 開 OBS 網址 ──
  await tab('settings');
  await waitFor(async () => ({ ok: await js(`document.querySelector('[data-role=obs-toggle]')?.disabled === false`) }), 10_000, 200);
  await js(`document.querySelector('[data-role=obs-toggle]').click(), true`);
  const urlR = await waitFor(async () => {
    const u = await js(`document.querySelector('[data-role=obs-url-history]')?.value ?? ''`);
    return { ok: /^http:\/\/127\.0\.0\.1:\d+\/history/.test(u), u };
  }, 15_000);
  if (!check('打開直播網址後顯示 /history 網址', urlR.ok, urlR.u)) return;
  const base = new URL(urlR.u).origin;
  const port = Number(new URL(urlR.u).port);
  const evil = await rawGet(port, '/events', 'evil.com');
  check('非本機 Host 被拒(403)', evil.includes('403'), evil);
  const trav = await rawGet(port, '/../Cargo.toml', `127.0.0.1:${port}`);
  check('路徑穿越被拒(404)', trav.includes('404'), trav);
  const page = await rawGet(port, '/history', `127.0.0.1:${port}`);
  check('/history 回 200', page.includes('200'), page);
  await screenshot(ui, '03-settings-obs.png');

  // ── 4. 開始一回合,TTS 供音;無頭 Edge 當 OBS 開兩頁 ──
  const edgeDir = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'edge-'));
  edgeProc = spawn(EDGE, ['--headless=new', `--remote-debugging-port=${EDGE_PORT}`, `--user-data-dir=${edgeDir}`, '--no-first-run', 'about:blank'], { stdio: 'ignore', windowsHide: true });
  await tab('main');
  // TTS 會循環唸;音訊來源只聽它(不受電腦上其他聲音影響),之後關掉它就是「沒聲音」
  const linesFile = path.join(OUT, 'tts-lines.txt');
  writeFileSync(linesFile, ['Good evening everyone, welcome back to the stream.', 'Today we are going to try a brand new strategy.', 'Thank you so much for watching, see you tomorrow.'].join('\n'));
  ttsProc = spawn('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(ROOT, '.claude', 'skills', 'verify', 'scripts', 'tts-window.ps1'), '-Lines', linesFile, '-GapMs', '1500'], { stdio: 'ignore', windowsHide: true });
  const srcSelect = `[...document.querySelectorAll('select')].find((s) => [...s.options].some((o) => o.value.startsWith('win:') || o.value === 'system'))`;
  const ttsOpt = await waitFor(async () => {
    await js(`(() => { const b = document.querySelector('button[aria-label="重新整理程式清單"]'); b && b.click(); return true; })()`);
    await sleep(1500);
    const v = await js(`[...${srcSelect}.options].find((o) => o.textContent.includes('[powershell.exe]: tingyi verify TTS'))?.value ?? ''`);
    return { ok: !!v, v };
  }, 30_000, 500);
  if (!check('音訊來源清單有 TTS 視窗', ttsOpt.ok)) return;
  await js(`(() => { const s = ${srcSelect}; Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, ${JSON.stringify(ttsOpt.v)}); s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  await sleep(1000);
  await js(`${startBtn}.click(), true`);
  const running = await waitFor(async () => ({ ok: await js(`!![...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止')`) }), 180_000, 500);
  if (!check('按開始後進入執行中', running.ok, `${Math.round(running.ms / 1000)}s`)) return;
  const sseP = sseFor(`${base}/events`, 45_000);
  const hist = await edgePage(`${base}/history?n=6&size=26`);
  const histJs = (e) => hist.evalJs(e);
  const got = await waitFor(async () => {
    const n = await histJs(`[...document.querySelectorAll('[data-role=translation]')].filter((x) => x.textContent && x.textContent !== '…').length`);
    return { ok: n >= 2, n };
  }, 120_000, 1000);
  check('OBS /history:出現至少 2 句有譯文的紀錄', got.ok, `n=${got.n}`);
  const histText = await histJs(`document.querySelector('[data-role=obs-history]')?.innerText ?? ''`);
  log(`/history 內容:${histText.replace(/\n/g, ' / ')}`);
  await screenshot(hist, '04-obs-history.png');
  // 中途重新整理(OBS 重新載入來源):重播本回合事件,句子還在
  await hist.send('Page.reload', {});
  await sleep(1500);
  const replay = await waitFor(async () => ({ ok: (await histJs(`document.querySelectorAll('[data-sentence-id]').length`)) >= 2 }), 15_000);
  check('OBS /history:重新整理後仍有先前句子(重播)', replay.ok);
  await hist.send('Page.navigate', { url: `${base}/subtitle?size=40` });
  await sleep(1500);
  const sub = await waitFor(async () => {
    const t = await histJs(`document.querySelector('[data-role=overlay-content]')?.innerText ?? ''`);
    return { ok: (t || '').trim().length > 0, t };
  }, 15_000);
  check('OBS /subtitle:顯示字幕', sub.ok, (sub.t || '').replace(/\n/g, ' / '));
  await screenshot(hist, '05-obs-subtitle.png');
  // 講完之後(預設 3 秒沒聲音)字幕收起;這時 OBS 重新整理來源,重播完仍應是收起狀態
  killTree(ttsProc?.pid, 'TTS powershell(停止說話)');
  const idle1 = await waitFor(async () => ({ ok: (await histJs(`document.querySelector('[data-role=overlay-content]')?.dataset.idle`)) === '1' }), 40_000, 500);
  check('OBS /subtitle:講完後字幕收起', idle1.ok, `${Math.round(idle1.ms / 1000)}s`);
  await hist.send('Page.reload', {});
  await sleep(2000);
  const idle2 = await waitFor(async () => ({ ok: (await histJs(`document.querySelector('[data-role=overlay-content]')?.dataset.idle`)) === '1' }), 5_000, 300);
  check('OBS /subtitle:閒置中重新整理後仍是收起(不卡住上一句)', idle2.ok);
  await screenshot(hist, '05b-obs-subtitle-idle-reload.png');
  const sse = await sseP;
  const types = sse.map((l) => {
    try {
      return JSON.parse(l).type;
    } catch {
      return '?';
    }
  });
  check('/events 第一則是 reset,之後有原文與譯文事件', types[0] === 'reset' && types.some((t) => /transcript/.test(t)) && types.some((t) => /translation/.test(t)), [...new Set(types)].join(','));
  writeFileSync(path.join(OUT, 'sse-sample.txt'), sse.slice(0, 80).join('\n'));
  await screenshot(ui, '06-main-running.png');
  // 關掉直播開關:已連上的 OBS 連線要斷,不再收到字幕
  const t0 = Date.now();
  const openConn = sseFor(`${base}/events`, 20_000).then(() => Date.now() - t0);
  await sleep(1000);
  await tab('settings');
  await waitFor(async () => ({ ok: await js(`document.querySelector('[data-role=obs-toggle]')?.checked === true`) }), 10_000, 200);
  await js(`document.querySelector('[data-role=obs-toggle]').click(), true`);
  const endedMs = await openConn;
  check('關掉直播開關後,已連上的 /events 連線 5 秒內結束', endedMs < 6000, `${endedMs} ms`);
  await tab('main');

  await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止').click(), true`);
  const stopped = await waitFor(async () => ({ ok: await js(`!!${startBtn}`) }), 60_000);
  check('停止後回到可開始', stopped.ok);
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  killTree(ttsProc?.pid, 'TTS powershell');
  killTree(edgeProc?.pid, 'msedge(headless)');
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
