import type { TranslateResponse } from '../shared/messages.ts';
import { loadSettings, saveSettings, type Settings } from '../shared/settings.ts';

const input = (id: string) => document.getElementById(id) as HTMLInputElement;
const status = document.getElementById('status') as HTMLDivElement;

function show(msg: string, ok: boolean): void {
  status.textContent = msg;
  status.className = `status ${ok ? 'ok' : 'bad'}`;
}

function fill(s: Settings): void {
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

void loadSettings().then(fill);
