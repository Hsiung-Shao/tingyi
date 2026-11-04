// 實跑驗證:YouTube 影片沒有字幕時,擴充自動請聽譯聽瀏覽器的聲音,字幕顯示在播放器裡。
//
// 用法(在專案根目錄):node .claude/skills/verify/scripts/verify_youtube_listen.mjs [--data D:\codeproject\translate\data]
// 1. 拒絕雙開;資料目錄用暫存目錄(bin、models 以 junction 指到 --data),預先寫入主視窗開始選項(1.8B、英文)。
// 2. 本機 HTTPS 樣本頁冒充 www.youtube.com(Chromium --host-resolver-rules + 自簽憑證):
//    #movie_player 的 getPlayerResponse() 沒有字幕軌,<video> 循環播放 TTS 英文。
// 3. `pnpm tauri dev` 啟動聽譯;Chromium 載入擴充,網站規則 youtube.com 開「沒字幕時用聽譯」。
// 4. 開觀看頁 → 聽譯自動開始(音訊來源 = 這個 Chromium、主視窗變執行中、用存下的 1.8B)→ 播放器內出現譯文。
// 5. 離開觀看頁 → 回合停止、音訊來源還原成系統聲音。
// 證據:evidence/<時間>-yt/,run.log、exit_code.txt、截圖、result.json。只關閉自己啟動的程序(依 PID)。
import { spawn, execFileSync, execSync } from 'node:child_process';
import { createServer } from 'node:https';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, existsSync, readdirSync, appendFileSync, createWriteStream } from 'node:fs';
import { tmpdir, homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import net from 'node:net';

const { values: opt } = parseArgs({ options: { data: { type: 'string', default: 'D:\\codeproject\\translate\\data' } } });
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..');
const stamp = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 15).replace(/^(\d{8})(\d{6}).*/, '$1-$2');
const OUT = path.join(ROOT, '.claude', 'skills', 'verify', 'evidence', `${stamp}-yt`);
mkdirSync(OUT, { recursive: true });
const EXT = path.join(ROOT, 'web', 'extension', 'dist');
const EXT_ID = 'jamiahhkhimbjajeddmncinemlmfkfhn';
const APP_PORT = 9339;
const CHROME_PORT = 9341;
const DEV_PORT = 1420;
const MODEL = 'Hy-MT2-1.8B-Q8_0';
const DATA = mkdtempSync(path.join(tmpdir(), 'tingyi-yt-data-'));
const WORK = mkdtempSync(path.join(tmpdir(), 'tingyi-yt-work-'));
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
async function liveText(page) {
  const doc = await page.send('DOM.getDocument', { depth: -1, pierce: true });
  let found = null;
  const walk = (n) => {
    if (found || !n) return;
    const a = n.attributes ?? [];
    const i = a.indexOf('class');
    if (i >= 0 && /\batx-live-text\b/.test(a[i + 1])) {
      found = n;
      return;
    }
    for (const c of [...(n.children ?? []), ...(n.shadowRoots ?? [])]) walk(c);
  };
  walk(doc.result?.root);
  return (found?.children ?? []).map((c) => c.nodeValue ?? '').join('');
}

function findChromium() {
  const base = path.join(process.env.LOCALAPPDATA ?? path.join(homedir(), 'AppData/Local'), 'ms-playwright');
  for (const d of readdirSync(base).filter((d) => /^chromium-\d+$/.test(d)).sort().reverse()) {
    const exe = path.join(base, d, 'chrome-win64', 'chrome.exe');
    if (existsSync(exe)) return exe;
  }
  throw new Error('找不到 ms-playwright Chromium');
}

const SPEECH = 'Welcome back to the channel. Today we are testing live subtitles for a video without captions. The local app listens to the browser and translates every sentence.';

// 冒充 YouTube 觀看頁:播放器物件的 getPlayerResponse() 沒有字幕軌
const WATCH = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Fixture: no-caption video</title></head>
<body style="margin:0;background:#0f0f0f;color:#fff;font:14px Roboto,Arial">
<div id="movie_player" style="position:relative;width:960px;height:540px;margin:24px;background:#000">
  <video src="/speech.wav" autoplay loop style="width:100%;height:100%"></video>
</div>
<a id="leave" href="/feed">home</a>
<script>document.getElementById('movie_player').getPlayerResponse = () => ({ videoDetails: { videoId: 'fixture01' } });</script>
</body></html>`;
// 有字幕軌的影片:字幕框照 YouTube 的結構,每 4 秒換一句英文
const WATCH_CC = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Fixture: captioned video</title></head>
<body style="margin:0;background:#0f0f0f;color:#fff;font:14px Roboto,Arial">
<div id="movie_player" style="position:relative;width:960px;height:540px;margin:24px;background:#000;font-size:11px">
<div class="ytp-caption-window-container"><div class="caption-window ytp-caption-window-bottom" style="position:absolute;bottom:2%;left:50%;width:640px;margin-left:-320px;text-align:center">
<span class="captions-text" style="display:block"><span class="caption-visual-line" style="display:block"><span id="seg" class="ytp-caption-segment" style="display:inline-block;background:rgba(8,8,8,0.75);font-size:28px;color:#fff;font-family:Roboto,Arial,sans-serif">Good evening and welcome to the show.</span></span></span>
</div></div></div>
<script>
document.getElementById('movie_player').getPlayerResponse = () => ({ videoDetails: { videoId: 'fixture-cc' }, captions: { playerCaptionsTracklistRenderer: { captionTracks: [{ kind: 'asr', languageCode: 'en' }] } } });
const lines = ['Good evening and welcome to the show.', 'Tonight we talk about the weather in the mountains.', 'Please remember to bring a warm jacket.'];
let i = 0;
setInterval(() => { i = (i + 1) % lines.length; document.getElementById('seg').textContent = lines[i]; }, 4000);
</script>
</body></html>`;
const FEED = `<!doctype html><html><head><meta charset="utf-8"><title>Fixture: feed</title></head><body><p>feed</p></body></html>`;

let devProc;
let chrome;
let server;
const startedPids = new Set();

async function main() {
  const busy = [];
  for (const p of [DEV_PORT, APP_PORT, CHROME_PORT]) if (await portInUse(p)) busy.push(p);
  const existing = pids('tingyi.exe');
  if (!check('沒有其他聽譯實例(拒絕雙開)', !busy.length && !existing.length, `埠占用 ${busy.join(',')}、tingyi.exe ${existing.join(',')}`)) return;
  const need = [path.join(opt.data, 'bin', 'llama', 'llama-server.exe'), path.join(opt.data, 'models', MODEL + '.gguf'), path.join(opt.data, 'models', 'silero_vad.onnx')];
  if (!check('llama-server、Hy-MT2 1.8B 與 VAD 就緒', need.every(existsSync), need.filter((p) => !existsSync(p)).join(', '))) return;
  for (const d of ['bin', 'models']) execFileSync('cmd', ['/c', 'mklink', '/J', path.join(DATA, d), path.join(opt.data, d)], { stdio: 'ignore' });
  // 主視窗存下的開始選項:擴充觸發時照這份開始(1.8B、英文,跑得快)
  const prefs = { model: MODEL, translate: true, asr_engine: 'sensevoice', lang: 'en', speaker_id: false, refine: false, glossary: '', replacements: '' };
  writeFileSync(path.join(DATA, 'settings.json'), JSON.stringify({ web_model: MODEL, prefs }, null, 2));
  check('隔離資料目錄與開始選項就緒', existsSync(path.join(DATA, 'models')), DATA);

  // TTS 音檔與自簽憑證
  const wav = path.join(WORK, 'speech.wav');
  execFileSync('powershell', ['-NoProfile', '-Command', `Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SelectVoice('Microsoft Zira Desktop'); $s.Rate = -1; $s.SetOutputToWaveFile('${wav}'); $s.Speak('${SPEECH}'); $s.Dispose()`]);
  execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=www.youtube.com', '-keyout', path.join(WORK, 'k.pem'), '-out', path.join(WORK, 'c.pem'), '-days', '1'], { stdio: 'ignore' });
  check('TTS 音檔與憑證產生', existsSync(wav) && existsSync(path.join(WORK, 'c.pem')));
  const wavBytes = readFileSync(wav);
  server = createServer({ key: readFileSync(path.join(WORK, 'k.pem')), cert: readFileSync(path.join(WORK, 'c.pem')) }, (q, s) => {
    if (q.url.startsWith('/speech.wav')) {
      s.writeHead(200, { 'Content-Type': 'audio/wav', 'Content-Length': wavBytes.length });
      return s.end(wavBytes);
    }
    s.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    s.end(q.url.startsWith('/watch?v=cc') ? WATCH_CC : q.url.startsWith('/watch') ? WATCH : FEED);
  }).listen(0, '127.0.0.1');
  await new Promise((r) => server.once('listening', r));
  const origin = `https://www.youtube.com:${server.address().port}`;

  log('建置擴充 …');
  execSync('pnpm --dir web --filter @atx/extension build', { cwd: ROOT, stdio: 'ignore' });
  check('擴充建置完成(含 YouTube 頁面世界腳本)', existsSync(path.join(EXT, 'yt-main.js')));

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
  const invoke = (cmd) => ui.evalJs(`window.__TAURI_INTERNALS__.invoke('${cmd}')`);
  const btn = (t) => ui.evalJs(`!![...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '${t}')`);
  const ready = await waitFor(async () => ({ ok: await btn('開始') }), 180_000);
  if (!check('主視窗就緒(開始可按)', ready.ok)) return;
  for (const p of pids('tingyi.exe')) startedPids.add(p);
  const rt = JSON.parse(readFileSync(path.join(DATA, 'runtime.json'), 'utf8'));
  check('runtime.json 有聽譯的 pipe 名稱', /^\\\\\.\\pipe\\tingyi-listen-[0-9a-f]{16}$/.test(rt.listenPipe ?? ''), rt.listenPipe);

  // ── 2. Chromium + 擴充,網站規則開「沒字幕時用聽譯」──
  const profile = mkdtempSync(path.join(tmpdir(), 'tingyi-yt-profile-'));
  const chromeArgs = [
    '--no-sandbox',
    `--user-data-dir=${profile}`,
    `--disable-extensions-except=${EXT}`,
    `--load-extension=${EXT}`,
    `--remote-debugging-port=${CHROME_PORT}`,
    '--host-resolver-rules=MAP www.youtube.com 127.0.0.1',
    '--ignore-certificate-errors',
    '--autoplay-policy=no-user-gesture-required',
    '--no-first-run',
    '--window-size=1280,900',
    'about:blank',
  ];
  chrome = spawn(findChromium(), chromeArgs, { env: ENV, stdio: 'ignore' });
  let sw;
  for (let i = 0; i < 60 && !sw; i++) {
    sw = (await cdpJson(CHROME_PORT, '/json/list')).find((t) => t.type === 'service_worker' && t.url.startsWith(`chrome-extension://${EXT_ID}/`));
    if (!sw) await sleep(500);
  }
  if (!check('擴充 service worker 啟動', !!sw)) return;
  const bg = connect(sw.webSocketDebuggerUrl);
  for (let i = 0; i < 40 && !(await bg.evalJs(`typeof chrome!=='undefined'&&!!chrome.storage`)); i++) await sleep(250);
  await bg.evalJs(`(async()=>{const k='atx:settings';const s=(await chrome.storage.local.get(k))[k]??{};s.sites={'youtube.com':{auto:false,listen:true}};await chrome.storage.local.set({[k]:s});return true})()`);
  check('網站規則:youtube.com 開「沒字幕時用聽譯」', true);
  const chromeRoot = chrome.pid;

  // ── 3. 開沒有字幕的觀看頁 → 聽譯自動開始 ──
  const t = await cdpJson(CHROME_PORT, `/json/new?${encodeURI(origin + '/watch?v=fixture01')}`);
  const page = connect(t.webSocketDebuggerUrl);
  await page.send('Page.enable');
  await page.send('Page.bringToFront');
  const playing = await waitFor(async () => ({ ok: await page.evalJs(`(() => { const v = document.querySelector('#movie_player video'); return !!v && !v.paused; })()`) }), 20_000);
  check('樣本影片在播放(沒有字幕軌)', playing.ok);
  const started = await waitFor(async () => ({ ok: await btn('停止') }), 180_000);
  check('擴充自動請聽譯開始(主視窗變成執行中)', started.ok, `${Math.round(started.ms / 1000)}s`);
  const src = await invoke('get_audio_source');
  check('音訊來源是這個 Chromium(瀏覽器主程序)', src?.source?.kind === 'app' && src.source.pid === chromeRoot && /chrome\.exe/i.test(src.source.exe), JSON.stringify(src?.source));
  const sub = await waitFor(async () => {
    const tx = await liveText(page);
    return { ok: /[\u4e00-\u9fff]/.test(tx ?? ''), tx };
  }, 120_000);
  check('播放器內出現聽譯的中文字幕', sub.ok, `${sub.tx ?? ''} ${Math.round(sub.ms / 1000)}s`);
  const leak = await page.evalJs(`(() => { const h = document.getElementById('atx-live'); return { shadow: !!h?.shadowRoot, text: document.getElementById('movie_player').innerText }; })()`);
  check('頁面腳本讀不到播放器字幕(closed shadow root)', !leak.shadow && !/[\u4e00-\u9fff]/.test(leak.text) && !leak.text.includes(sub.tx ?? '\u0000'), JSON.stringify(leak).slice(0, 120));
  await screenshot(page, '01-player-live-subtitles.png');
  await screenshot(ui, '02-app-running-from-extension.png');
  const llama = (await ui.evalJs(`window.__TAURI_INTERNALS__.invoke('web_status')`)) ?? {};
  check('翻譯用主視窗存下的模型(1.8B)', llama.server_model === MODEL && llama.subtitles === true, JSON.stringify(llama));

  // ── 4. 從主視窗停止 → 字幕框收起;再播放 → 同一分頁重新開始 ──
  const sentences = () => ui.evalJs(`document.querySelectorAll('[data-sentence-id]').length`);
  const before = await sentences();
  await ui.evalJs(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === '停止').click(), true`);
  const manualStop = await waitFor(async () => ({ ok: (await btn('開始')) && !(await page.evalJs(`!!document.getElementById('atx-live')`)) }), 60_000);
  check('主視窗按停止:回合結束、播放器字幕框收起', manualStop.ok, `${Math.round(manualStop.ms / 1000)}s`);
  await page.evalJs(`(async () => { const v = document.querySelector('#movie_player video'); v.pause(); await new Promise((r) => setTimeout(r, 300)); await v.play(); return true; })()`);
  const again = await waitFor(async () => ({ ok: await btn('停止') }), 120_000);
  check('同一分頁再播放:聽譯重新開始', again.ok, `${Math.round(again.ms / 1000)}s`);
  const sub2 = await waitFor(async () => {
    const tx = await liveText(page);
    return { ok: /[一-鿿]/.test(tx ?? ''), tx };
  }, 120_000);
  check('重新開始後播放器再出現字幕', sub2.ok, sub2.tx ?? '');
  const after = await sentences();
  check('擴充重新開始沒有清掉主視窗的字幕紀錄', after >= before && before > 0, `之前 ${before} 句、之後 ${after} 句`);

  // ── 5. 另一個分頁開始播放沒字幕的影片 → 改由新分頁聽(舊回合停完再開新的)──
  await page.evalJs(`document.querySelector('#movie_player video').pause(), true`);
  const t2 = await cdpJson(CHROME_PORT, `/json/new?${encodeURI(origin + '/watch?v=fixture02')}`);
  const page2 = connect(t2.webSocketDebuggerUrl);
  await page2.send('Page.enable');
  await page2.send('Page.bringToFront');
  const sub3 = await waitFor(async () => {
    const tx = await liveText(page2);
    return { ok: /[一-鿿]/.test(tx ?? ''), tx };
  }, 120_000);
  const oldBox = await page.evalJs(`!!document.getElementById('atx-live')`);
  check('換分頁:新分頁出現字幕、舊分頁字幕框收起', sub3.ok && !oldBox, `${sub3.tx ?? ''} ${Math.round(sub3.ms / 1000)}s 舊分頁框=${oldBox}`);
  await screenshot(page2, '03-second-tab-subtitles.png');

  // ── 6. 離開觀看頁 → 回合停止、來源還原 ──
  await page2.evalJs(`document.getElementById('leave').click(), true`);
  const stopped = await waitFor(async () => ({ ok: await btn('開始') }), 60_000);
  check('離開觀看頁後聽譯回合停止', stopped.ok, `${Math.round(stopped.ms / 1000)}s`);
  const src2 = await invoke('get_audio_source');
  check('音訊來源還原成系統聲音', src2?.source?.kind === 'system', JSON.stringify(src2?.source));

  // ── 7. 有字幕的影片 + 翻譯影片字幕:只顯示譯文,原文字幕不顯示;有字幕不會啟動聽譯 ──
  await bg.evalJs(`(async()=>{const k='atx:settings';const s=(await chrome.storage.local.get(k))[k]??{};s.sites={'youtube.com':{auto:false,listen:true,captions:true}};await chrome.storage.local.set({[k]:s});return true})()`);
  await page2.send('Page.navigate', { url: origin + '/watch?v=cc' });
  await sleep(1500);
  await page2.send('Page.bringToFront');
  const cc = await waitFor(async () => {
    const r = await page2.evalJs(`(() => { const t = document.querySelector('.caption-window font.atx-caption'); const o = document.querySelector('.captions-text'); return { tr: t?.textContent ?? '', origDisplay: o ? getComputedStyle(o).display : null }; })()`);
    return { ok: /[\u4e00-\u9fff]/.test(r.tr) && r.origDisplay === 'none', r };
  }, 90_000);
  check('翻譯影片字幕:只顯示譯文、原文字幕不顯示', cc.ok, JSON.stringify(cc.r));
  const changed = await waitFor(async () => {
    const r = await page2.evalJs(`(() => ({ src: document.getElementById('seg').textContent, tr: document.querySelector('.caption-window font.atx-caption')?.textContent ?? '', origDisplay: getComputedStyle(document.querySelector('.captions-text')).display }))()`);
    return { ok: r.tr !== cc.r?.tr && /[\u4e00-\u9fff]/.test(r.tr) && r.origDisplay === 'none', r };
  }, 30_000, 500);
  check('換句後顯示新句的譯文,原文仍不顯示', changed.ok, JSON.stringify(changed.r));
  await screenshot(page2, '05-captioned-translation-only.png');
  // 不送翻的字幕(已是中文)照常顯示原文,不會整個消失
  await page2.evalJs(`(() => { const s = document.getElementById('seg'); s.textContent = '這一句本來就是中文字幕'; return true; })()`);
  const zh = await waitFor(async () => {
    const r = await page2.evalJs(`(() => ({ display: getComputedStyle(document.querySelector('.captions-text')).display, state: document.querySelector('.caption-window').dataset.atxState ?? null }))()`);
    return { ok: r.display !== 'none', r };
  }, 5_000, 200);
  check('不送翻的中文字幕照常顯示原文', zh.ok, JSON.stringify(zh.r));
  check('有字幕的影片不啟動聽譯', await btn('開始'));
  await screenshot(ui, '04-app-stopped.png');
}

try {
  await main();
} catch (e) {
  check('驗證流程未拋例外', false, String(e?.stack ?? e));
} finally {
  chrome?.kill();
  try {
    const rt = JSON.parse(readFileSync(path.join(DATA, 'runtime.json'), 'utf8'));
    if (rt?.pid) startedPids.add(rt.pid);
  } catch {
    // 程式結束時會刪掉 runtime.json
  }
  for (const p of startedPids) killTree(p, 'tingyi.exe');
  killTree(devProc?.pid, 'pnpm tauri dev');
  server?.close();
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
