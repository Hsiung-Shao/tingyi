import type { LinkStatus } from '../background/appLink.ts';
import type { TranslateResponse } from '../shared/messages.ts';
import { loadSettings, saveSettings, type Settings } from '../shared/settings.ts';

const input = (id: string) => document.getElementById(id) as HTMLInputElement;
const status = document.getElementById('status') as HTMLDivElement;

function show(msg: string, ok: boolean): void {
  status.textContent = msg;
  status.className = `status ${ok ? 'ok' : 'bad'}`;
}

/** 自動連線時端點由本機程式提供,這些欄位唯讀 */
const MANAGED = ['bridgeUrl', 'bridgeToken', 'baseUrl', 'model', 'apiKey', 'format', 'jsonMode'];
const connection = document.getElementById('connection') as HTMLSelectElement;

function applyConnection(): void {
  for (const id of MANAGED) (document.getElementById(id) as HTMLInputElement).disabled = connection.value === 'auto';
}

function fill(s: Settings): void {
  connection.value = s.connection;
  applyConnection();
  input('bridgeUrl').value = s.bridge.url;
  input('bridgeToken').value = s.bridge.token;
  input('bridgeModel').value = s.bridge.model;
  input('baseUrl').value = s.direct.baseUrl;
  input('model').value = s.direct.model;
  input('apiKey').value = s.direct.apiKey;
  (document.getElementById('format') as HTMLSelectElement).value = s.direct.format;
  input('jsonMode').checked = s.direct.jsonMode;
  input('extraBody').value = s.direct.extraBody;
  input('bridgeSeg').value = String(s.limits.bridge.maxSegments);
  input('bridgeChars').value = String(s.limits.bridge.maxChars);
  input('directSeg').value = String(s.limits.direct.maxSegments);
  input('directChars').value = String(s.limits.direct.maxChars);
  input('concurrency').value = String(s.concurrency);
}

async function collect(): Promise<Settings> {
  const s = await loadSettings();
  const extraBody = input('extraBody').value.trim() || '{}';
  JSON.parse(extraBody); // 格式錯就讓呼叫端顯示錯誤
  const num = (id: string, min: number) => Math.max(min, Number(input(id).value) || min);
  return {
    ...s,
    connection: connection.value as Settings['connection'],
    bridge: { ...s.bridge, url: input('bridgeUrl').value.trim(), token: input('bridgeToken').value.trim(), model: input('bridgeModel').value.trim() },
    direct: {
      baseUrl: input('baseUrl').value.trim().replace(/\/$/, ''),
      model: input('model').value.trim(),
      apiKey: input('apiKey').value.trim(),
      format: (document.getElementById('format') as HTMLSelectElement).value as Settings['direct']['format'],
      jsonMode: input('jsonMode').checked,
      extraBody,
    },
    limits: {
      bridge: { maxSegments: num('bridgeSeg', 1), maxChars: num('bridgeChars', 200) },
      direct: { maxSegments: num('directSeg', 1), maxChars: num('directChars', 200) },
    },
    concurrency: Math.min(8, num('concurrency', 1)),
  };
}

document.getElementById('save')!.addEventListener('click', async () => {
  try {
    await saveSettings(await collect());
    show('已儲存', true);
  } catch (e) {
    show(`儲存失敗:${e instanceof Error ? e.message : String(e)}`, false);
  }
});

document.getElementById('test')!.addEventListener('click', async () => {
  try {
    await saveSettings(await collect());
    show('測試中…', true);
    const started = performance.now();
    const res = (await chrome.runtime.sendMessage({
      type: 'translate',
      segments: [{ id: 1, text: `Hello! This is a connection test (${Date.now()}).` }],
      context: { title: 'connection test' },
    })) as TranslateResponse;
    const ms = Math.round(performance.now() - started);
    if (res.ok && res.translations[0]) show(`成功(${ms} ms):${res.translations[0].text}`, true);
    else show(`失敗:${res.ok ? '沒有拿到譯文' : res.error}`, false);
  } catch (e) {
    show(`失敗:${e instanceof Error ? e.message : String(e)}`, false);
  }
});

connection.addEventListener('change', async () => {
  const s = await loadSettings();
  s.connection = connection.value as Settings['connection'];
  await saveSettings(s);
  applyConnection();
  if (s.connection === 'auto') await relink();
});

/** 向本機程式取端點,成功就把新值填回欄位 */
async function relink(): Promise<void> {
  const box = document.getElementById('linkState') as HTMLDivElement;
  const link = (await chrome.runtime.sendMessage({ type: 'appLink' })) as LinkStatus;
  if (link.state === 'connected') {
    box.textContent = link.status === 'ready' ? '已連上本機程式,端點已自動更新' : `已連上本機程式,模型狀態:${link.status}`;
    box.className = 'status ok';
    fill(await loadSettings());
  } else if (link.state === 'not-installed') {
    box.textContent = '找不到本機程式,請先安裝,或改用手動設定';
    box.className = 'status bad';
  } else if (link.state === 'app-error') {
    box.textContent = `本機程式沒有回應:${link.message}`;
    box.className = 'status bad';
  } else box.textContent = '';
}

void loadSettings().then(async (s) => {
  fill(s);
  if (s.connection === 'auto') await relink();
});
