// 實跑驗證:遊戲術語包(本機 POE DB → scripts/poe-terms/build_term_pack.py → data/terms/<game>.json)。
// 沒有術語包時字幕頁不顯示「遊戲術語」;產生後出現 PoE1/PoE2;PoE1 + SenseVoice 時 TTS 唸 POE 句子,
// 譯文用官方譯名、「Path of Excel」原文被修正;Whisper 引擎送出熱詞(app 紀錄)。
// 需要本機 POE DB(d:\codeproject\Pob2\pob-zh-engine\tools\ggpk_zh / ggpk2_zh)。TTS 會出聲。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_poe_terms.mjs
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync, copyFileSync, readFileSync, cpSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const argData = process.argv.indexOf('--data');
const SRC_DATA = argData > 0 ? process.argv[argData + 1] : 'D:\\codeproject\\translate\\data';
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-poe-terms`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
mkdirSync('D:\\tingyi-verify-tmp', { recursive: true });
const DATA = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'poeterms-'));
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
const MT = 'Hy-MT2-1.8B-Q4_K_M';
let devProc, ttsProc;
const startedPids = new Set();
const LINES = [
  'I just found a Chaos Orb in Wraeclast.',
  'The Juggernaut is going to fight Kitava tonight.',
  'Welcome back to Path of Excel, everyone.',
];

async function launch(tag) {
  log(`啟動 pnpm tauri dev(${tag})…`);
  const appLog = createWriteStream(path.join(OUT, `app-${tag}.log`));
  devProc = spawn('pnpm tauri dev', { shell: true, cwd: ROOT, env: ENV, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  devProc.stdout.pipe(appLog);
  devProc.stderr.pipe(appLog);
  const appT = await waitFor(async () => {
    const t = (await cdpJson(APP_PORT, '/json/list', 1)).find((x) => x.type === 'page' && x.url.startsWith(`http://localhost:${DEV_PORT}/`) && !x.url.includes('overlay'));
    return { ok: !!t, t };
  }, 600_000, 2000);
  if (!appT.ok) return null;
  for (const p of pids('tingyi.exe')) startedPids.add(p);
  const ui = connect(appT.t.webSocketDebuggerUrl);
  await ui.send('Page.enable');
  return ui;
}

async function stopApp() {
  for (const p of startedPids) killTree(p, 'tingyi.exe');
  startedPids.clear();
  killTree(devProc?.pid, 'pnpm tauri dev');
  await waitFor(async () => ({ ok: !(await portInUse(DEV_PORT)) && !(await portInUse(APP_PORT)) }), 30_000, 500);
}

/** 選音訊來源為 TTS 視窗、選引擎、開始,等句子;回 [{source, translation}]。 */
async function runSession(ui, { engine, pack, want, tag }) {
  const js = (e) => ui.evalJs(e);
  const sel = (q) => `[...document.querySelectorAll('select')].find((s) => ${q})`;
  const set = async (selector, value) =>
    js(`(() => { const s = ${selector}; if (!s) return false; Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, ${JSON.stringify(value)}); s.dispatchEvent(new Event('change', { bubbles: true })); return s.value; })()`);
  await set(sel(`[...s.options].some((o) => o.value === 'whisper-turbo')`), engine);
  await set(`document.querySelector('#term-pack')`, pack);
  await set(sel(`[...s.options].some((o) => o.value === '${MT}')`), MT);
  // 來源語言鎖英文(TTS 唸英文;避免自動偵測誤判)
  await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === 'en')?.click(), true`);
  const srcSel = `document.querySelector('select[aria-label="音訊來源"]')`;
  const ttsOpt = await waitFor(async () => {
    await js(`(() => { const b = document.querySelector('button[aria-label="重新整理程式清單"]'); b && b.click(); return true; })()`);
    await sleep(1200);
    const v = await js(`[...${srcSel}.options].find((o) => o.textContent.includes('[powershell.exe]: tingyi verify TTS'))?.value ?? ''`);
    return { ok: !!v, v };
  }, 30_000, 500);
  if (!check(`${tag}:音訊來源清單有 TTS 視窗`, ttsOpt.ok)) return [];
  await set(srcSel, ttsOpt.v);
  await sleep(800);
  const startBtn = `[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '開始')`;
  const can = await waitFor(async () => ({ ok: await js(`!!${startBtn} && !${startBtn}.disabled`) }), 30_000);
  if (!check(`${tag}:可以開始`, can.ok, await js(`document.querySelector('[data-role=start-missing]')?.textContent ?? ''`))) return [];
  await js(`${startBtn}.click(), true`);
  // 上一輪有紀錄時會問清除或保留:清除
  await sleep(500);
  await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '清除')?.click(), true`);
  const running = await waitFor(async () => ({ ok: await js(`!![...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止')`) }), 240_000, 500);
  if (!check(`${tag}:進入執行中`, running.ok, `${Math.round(running.ms / 1000)}s`)) return [];
  const got = await waitFor(async () => {
    const rows = await js(`[...document.querySelectorAll('[data-sentence-id]')].map((r) => ({ source: r.querySelector('[data-role=source]')?.textContent ?? '', translation: r.querySelector('div.text-base')?.textContent ?? '' }))`);
    const done = rows.filter((r) => r.translation && r.translation !== '…');
    // want 是數字:收到這麼多句;是陣列:每個關鍵字都要出現在某句原文(TTS 循環,開頭可能只收到半句)
    const ok = Array.isArray(want) ? want.every((k) => done.some((r) => new RegExp(k, 'i').test(r.source))) : done.length >= want;
    return { ok, rows: done };
  }, 150_000, 1000);
  await screenshot(ui, `${tag}.png`);
  await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止')?.click(), true`);
  await waitFor(async () => ({ ok: await js(`!!${startBtn}`) }), 60_000);
  for (const r of got.rows ?? []) log(`${tag}  原文:${r.source}  譯文:${r.translation}`);
  return got.rows ?? [];
}

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  // 資料目錄:SenseVoice、VAD、llama.cpp、1.8B Q4、Whisper(伺服器 + 模型);先不放術語包
  mkdirSync(path.join(DATA, 'models', SV_DIR), { recursive: true });
  for (const f of ['model.int8.onnx', 'tokens.txt']) copyFileSync(path.join(SRC_DATA, 'models', SV_DIR, f), path.join(DATA, 'models', SV_DIR, f));
  copyFileSync(path.join(SRC_DATA, 'models', 'silero_vad.onnx'), path.join(DATA, 'models', 'silero_vad.onnx'));
  copyFileSync(path.join(SRC_DATA, 'models', `${MT}.gguf`), path.join(DATA, 'models', `${MT}.gguf`));
  copyFileSync(path.join(SRC_DATA, 'models', 'ggml-large-v3-turbo-q8_0.bin'), path.join(DATA, 'models', 'ggml-large-v3-turbo-q8_0.bin'));
  cpSync(path.join(SRC_DATA, 'bin', 'llama'), path.join(DATA, 'bin', 'llama'), { recursive: true });
  cpSync(path.join(SRC_DATA, 'bin', 'whisper'), path.join(DATA, 'bin', 'whisper'), { recursive: true });
  log('資料目錄準備完成(不含術語包)');

  // TTS:循環唸 POE 句子,音訊來源只聽它
  const linesFile = path.join(OUT, 'tts-lines.txt');
  writeFileSync(linesFile, LINES.join('\n'));
  ttsProc = spawn('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(ROOT, '.claude', 'skills', 'verify', 'scripts', 'tts-window.ps1'), '-Lines', linesFile, '-GapMs', '1200'], { stdio: 'ignore', windowsHide: true });

  // ── 1. 沒有術語包:不顯示「遊戲術語」 ──
  let ui = await launch('nopack');
  if (!check('主視窗頁面出現在 CDP', !!ui)) return;
  await waitFor(async () => ({ ok: await ui.evalJs(`!!document.querySelector('[data-tab=main]')`) }), 60_000);
  await sleep(1500);
  check('沒有術語包時字幕頁不顯示「遊戲術語」', !(await ui.evalJs(`!!document.querySelector('[data-role=term-pack]')`)));

  // ── 2. 產生術語包後重新載入:出現 PoE1 / PoE2 ──
  for (const g of ['poe1', 'poe2']) {
    const out = execFileSync('python', [path.join(ROOT, 'scripts', 'poe-terms', 'build_term_pack.py'), '--game', g, '--out', path.join(DATA, 'terms')], { encoding: 'utf8', env: { ...process.env, PYTHONIOENCODING: 'utf-8' } });
    log(out.trim());
  }
  await ui.send('Page.reload', {});
  const opts = await waitFor(async () => {
    const o = await ui.evalJs(`[...(document.querySelector('#term-pack')?.options ?? [])].map((x) => x.value + '|' + x.textContent)`);
    return { ok: o.length === 3, o };
  }, 30_000, 500);
  check('產生術語包後出現「遊戲術語」:不使用 / poe1 / poe2', opts.ok && opts.o.some((x) => x.startsWith('poe1|')) && opts.o.some((x) => x.startsWith('poe2|')), JSON.stringify(opts.o));
  await screenshot(ui, '01-term-pack-select.png');

  // ── 3. PoE1 + SenseVoice:官方譯名與誤聽修正 ──
  const rows = await runSession(ui, { engine: 'sensevoice', pack: 'poe1', want: 3, tag: '02-sensevoice-poe1' });
  const appLog = () => readFileSync(path.join(OUT, 'app-nopack.log'), 'utf8');
  check('app 紀錄有載入 poe1 術語包', /遊戲術語包 poe1(.*):\d+ 詞/.test(appLog()), (appLog().match(/遊戲術語包 poe1[^\n]*/) || [''])[0]);
  const pathRow = rows.find((r) => /path of/i.test(r.source));
  check('「Path of Excel」原文被修正成 Path of Exile', !!pathRow && /Path of Exile/.test(pathRow.source) && !/excel/i.test(pathRow.source), pathRow?.source ?? '沒有含 path of 的句子');
  // 使用者指定 Path of Exile 保留英文(overrides 中文欄填英文)
  check('Path of Exile 譯文保留英文', !!pathRow && /Path of Exile/i.test(pathRow.translation) && !pathRow.translation.includes('流亡黯道'), pathRow?.translation ?? '');

  // ── 4. Whisper 引擎:送出熱詞 ──
  // 等三句都完整收到(TTS 循環唸,開頭可能只收到半句)
  const wrows = await runSession(ui, { engine: 'whisper-turbo', pack: 'poe1', want: ['found', 'fight', 'welcome'], tag: '03-whisper-poe1' });
  const hot = appLog().match(/Whisper 熱詞 (\d+) 個/);
  check('Whisper 引擎送出術語包熱詞', !!hot && Number(hot[1]) > 10, hot?.[0] ?? '紀錄沒有 Whisper 熱詞');
  check('Whisper 引擎三句都辨識出來', ['found', 'fight', 'welcome'].every((k) => wrows.some((r) => new RegExp(k, 'i').test(r.source))), `${wrows.length} 句`);

  // 官方譯名:原文「辨識對了」的術語,譯文都要是官方名;兩個引擎合計至少辨識對 3 個不同術語。
  // 原文辨識錯(SenseVoice 把 Kitava 聽成 Ki of it)時術語包對不到,屬辨識問題,另列在 run.log。
  const TERMS = { 'chaos orb': '混沌石', wraeclast: '瓦爾克拉斯', juggernaut: '勇士', kitava: '奇塔弗' };
  const recognized = new Set();
  const wrong = [];
  for (const r of [...rows, ...wrows]) {
    for (const [en, zh] of Object.entries(TERMS)) {
      if (!new RegExp(`\\b${en}`, 'i').test(r.source)) continue;
      recognized.add(en);
      if (!r.translation.includes(zh)) wrong.push(`${en}→「${r.translation}」`);
    }
  }
  check('辨識對的術語都譯成官方名', wrong.length === 0, wrong.join(';') || `辨識對 ${[...recognized].join('、')}`);
  check('兩個引擎合計至少辨識對 3 個不同術語', recognized.size >= 3, [...recognized].join('、'));
  const missed = Object.keys(TERMS).filter((en) => !rows.some((r) => new RegExp(`\\b${en}`, 'i').test(r.source)));
  log(`SenseVoice 沒辨識對的術語(辨識問題,術語包無法補):${missed.join('、') || '無'}`);
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  killTree(ttsProc?.pid, 'TTS powershell');
  await stopApp();
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
