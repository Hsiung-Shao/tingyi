// 實跑驗證:新電腦上「辨識引擎」下拉選單的每個引擎都能在 app 內下載並開始。
// SenseVoice 與 VAD 從本機 data 複製(這兩個由 verify_fresh_install.mjs 驗下載);
// Whisper(官方 CUDA 版 + 模型)、Qwen3-ASR 0.6B / 1.7B、Cohere 實際從 GitHub / Hugging Face / ModelScope 下載,約 8 GB。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_engine_downloads.mjs [--data <本機資料目錄>]
// 暫存資料目錄放 D:\tingyi-verify-tmp(C 槽空間不夠),跑完刪除。只關閉自己啟動的程序(依 PID)。
import { spawn, execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, existsSync, statSync, appendFileSync, createWriteStream, rmSync, copyFileSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const argData = process.argv.indexOf('--data');
const SRC_DATA = argData > 0 ? process.argv[argData + 1] : 'D:\\codeproject\\translate\\data';
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-engines`);
mkdirSync(OUT, { recursive: true });
const APP_PORT = 9339;
const DEV_PORT = 1420;
mkdirSync('D:\\tingyi-verify-tmp', { recursive: true });
const DATA = mkdtempSync(path.join('D:\\tingyi-verify-tmp', 'engines-'));
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
const ENGINES = [
  { id: 'whisper-turbo', ids: ['whisper-server', 'whisper-turbo-model'], files: ['bin/whisper/Release/whisper-server.exe', 'models/whisper/ggml-large-v3-turbo-q8_0.bin'] },
  { id: 'qwen3-0.6b', ids: ['qwen3-0.6b'], files: ['models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/decoder.int8.onnx', 'models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25/tokenizer/vocab.json'] },
  { id: 'qwen3-1.7b', ids: ['qwen3-1.7b'], files: ['models/qwen3-asr-onnx-modelscope/model_1.7B/decoder.int8.onnx', 'models/qwen3-asr-onnx-modelscope/tokenizer/vocab.json'] },
  { id: 'cohere', ids: ['cohere'], files: ['models/sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01/encoder.int8.onnx.data'] },
];
let devProc;
const startedPids = new Set();

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  // 新電腦:只放 SenseVoice 與 VAD(第一次使用必下的基本模型),其他什麼都沒有
  mkdirSync(path.join(DATA, 'models', SV_DIR), { recursive: true });
  for (const f of ['model.int8.onnx', 'tokens.txt']) copyFileSync(path.join(SRC_DATA, 'models', SV_DIR, f), path.join(DATA, 'models', SV_DIR, f));
  copyFileSync(path.join(SRC_DATA, 'models', 'silero_vad.onnx'), path.join(DATA, 'models', 'silero_vad.onnx'));
  check('暫存資料目錄只有基本模型(沒有 bin、沒有其他引擎)', !existsSync(path.join(DATA, 'bin')), DATA);

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
  const engineSelect = `[...document.querySelectorAll('select')].find((s) => [...s.options].some((o) => o.value === 'whisper-turbo'))`;
  const choose = (id) =>
    js(`(() => { const s = ${engineSelect}; Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, '${id}'); s.dispatchEvent(new Event('change', { bubbles: true })); return s.value; })()`);
  const status = () => js(`document.querySelector('header span[title]')?.textContent ?? ''`);

  // 先關掉本機翻譯(這個旅程只驗辨識引擎;翻譯伺服器的下載另有旅程)
  await waitFor(async () => ({ ok: await js(`!!${engineSelect}`) }), 60_000);
  await js(`(() => { const l = [...document.querySelectorAll('label')].find((x) => x.textContent.includes('啟用本機翻譯')); const i = l && l.querySelector('input'); if (i && i.checked) i.click(); return true; })()`);
  const opts = await js(`[...${engineSelect}.options].map((o) => ({ v: o.value, d: o.disabled, t: o.textContent }))`);
  check('下拉選單列出 6 個引擎且都能選', opts.length === 6 && opts.every((o) => !o.d), JSON.stringify(opts.map((o) => o.v)));
  check('沒下載的引擎標「(未下載)」', ENGINES.every((e) => opts.find((o) => o.v === e.id)?.t.includes('未下載')));

  // ── 1. 每個引擎:選了就出現對應下載卡、開始不能按;下載到已下載 ──
  for (const e of ENGINES) {
    await choose(e.id);
    const card = await waitFor(async () => ({ ok: await js(`${JSON.stringify(e.ids)}.every((id) => !!document.querySelector('[data-group=engine] [data-model-id="' + id + '"]'))`) }), 30_000);
    check(`選 ${e.id}:下方出現它的下載卡`, card.ok);
    const blocked = await waitFor(async () => {
      const h = await hint();
      return { ok: (await js(`!!${startBtn} && ${startBtn}.disabled`)) && /的模型/.test(h), h };
    }, 15_000);
    check(`選 ${e.id}:沒下載前「開始」不能按並說缺模型`, blocked.ok, blocked.h);
    await screenshot(ui, `01-${e.id}-card.png`);
    for (const id of e.ids) {
      await js(`(() => { const b = [...document.querySelectorAll('[data-model-id="${id}"] button')].find((x) => /下載|繼續/.test(x.textContent)); b && b.click(); return !!b; })()`);
    }
    const dl = await waitFor(async () => {
      const err = await js(`[...document.querySelectorAll('[data-group=engine] [data-role=model-error]')].map((x) => x.textContent).join(' | ')`);
      if (err) return { ok: false, err, stop: true };
      return { ok: await js(`${JSON.stringify(e.ids)}.every((id) => !!document.querySelector('[data-model-id="' + id + '"] [data-role=model-ready]'))`) };
    }, 3_600_000, 5000);
    check(`${e.id}:在 app 內下載完(卡片顯示已下載)`, dl.ok, `${Math.round(dl.ms / 1000)}s ${dl.err ?? ''}`);
    const missingFiles = e.files.filter((f) => !existsSync(path.join(DATA, f)));
    check(`${e.id}:檔案在預期位置`, !missingFiles.length, missingFiles.join(', ') || e.files.join(', '));
    await screenshot(ui, `02-${e.id}-downloaded.png`);
  }

  // ── 2. 每個引擎實際開始一次(Cohere 先指定語言) ──
  for (const e of ENGINES) {
    await choose(e.id);
    if (e.id === 'cohere') {
      const blocked = await waitFor(async () => ({ ok: /來源語言/.test(await hint()) && !/下載卡/.test(await hint()) && (await js(`${startBtn}.disabled`)) }), 15_000);
      check('cohere:語言自動時不能開始並提示要指定語言', blocked.ok, await hint());
      await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === 'ja').click(), true`);
    }
    const can = await waitFor(async () => ({ ok: (await js(`!!${startBtn} && !${startBtn}.disabled`)) && (await hint()) === '' }), 30_000);
    check(`${e.id}:下載後「開始」可以按`, can.ok, await hint());
    if (!can.ok) continue;
    await js(`${startBtn}.click(), true`);
    const seen = new Set();
    const running = await waitFor(async () => {
      seen.add(await status());
      return { ok: await js(`!![...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止')`) };
    }, 300_000, 300);
    result.statuses[e.id] = [...seen];
    check(`${e.id}:按開始後進入執行中`, running.ok, `${Math.round(running.ms / 1000)}s;狀態 ${[...seen].join(' / ')}`);
    if (e.id === 'whisper-turbo') {
      // 有 CUDA 快取時預熱只要約 0.3 秒,狀態列取樣可能錯過,以 app 紀錄為準
      const warmLine = readFileSync(path.join(OUT, 'app.log'), 'utf8').match(/Whisper 預熱完成 \d+ ms/)?.[0];
      check('whisper-turbo:有經過預熱步驟', !!warmLine, `${warmLine ?? '紀錄沒有預熱完成'};狀態 ${[...seen].join(' / ')}`);
    }
    await screenshot(ui, `03-${e.id}-running.png`);
    if (running.ok) {
      await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止').click(), true`);
      const stopped = await waitFor(async () => ({ ok: await js(`!!${startBtn}`) }), 60_000);
      check(`${e.id}:停止後回到可開始`, stopped.ok);
    }
    if (e.id === 'cohere') await js(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '自動').click(), true`);
  }
  for (const f of ['bin/whisper/Release/whisper-server.exe', 'models/whisper/ggml-large-v3-turbo-q8_0.bin']) {
    const p = path.join(DATA, f);
    log(`${f}:${existsSync(p) ? statSync(p).size : '不存在'}`);
  }
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
