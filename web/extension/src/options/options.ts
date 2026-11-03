import type { LinkStatus } from '../background/appLink.ts';
import type { TranslateResponse } from '../shared/messages.ts';
import { loadSettings, saveSettings, type Settings } from '../shared/settings.ts';
import { isYouTube, updateSiteRule, type SiteRule } from '../shared/sites.ts';

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
      // slot 由本機程式指派,不在選項頁編輯
      slots: s.direct.slots,
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

/** 網站規則清單:每條可切換、刪除;改了立刻存(各分頁由 storage.onChanged 套用) */
function renderSites(sites: Settings['sites']): void {
  const box = document.getElementById('sites') as HTMLDivElement;
  const keys = Object.keys(sites).sort();
  if (keys.length === 0) {
    box.innerHTML = '<p class="muted">還沒有設定任何網站。</p>';
    return;
  }
  box.replaceChildren(
    ...keys.map((key) => {
      const rule = sites[key]!;
      const row = document.createElement('div');
      row.className = 'row';
      const name = document.createElement('span');
      name.textContent = key;
      const check = (label: string, field: keyof SiteRule) => {
        const l = document.createElement('label');
        const c = document.createElement('input');
        c.type = 'checkbox';
        c.checked = rule[field] === true;
        c.addEventListener('change', () => void editSite(key, { [field]: c.checked }));
        l.append(c, ` ${label}`);
        return l;
      };
      const del = document.createElement('button');
      del.className = 'secondary';
      del.textContent = '刪除';
      del.style.flex = '0';
      del.addEventListener('click', () => void editSite(key, { auto: false, captions: false, listen: false }));
      row.append(name, check('自動翻譯', 'auto'), ...(isYouTube(key) ? [check('翻譯字幕', 'captions')] : []), del);
      return row;
    }),
  );
}

async function editSite(key: string, patch: Partial<SiteRule>): Promise<void> {
  const s = await loadSettings();
  s.sites = updateSiteRule(s.sites, key, patch);
  await saveSettings(s);
  renderSites(s.sites);
}

document.getElementById('addSite')!.addEventListener('click', () => {
  const el = input('newSite');
  // 貼網址也行:只取網域
  const key = el.value.trim().replace(/^https?:\/\//, '').replace(/\/.*$/, '').replace(/^www\./, '').toLowerCase();
  if (!key) return;
  el.value = '';
  void editSite(key, { auto: true });
});

void loadSettings().then(async (s) => {
  fill(s);
  renderSites(s.sites);
  if (s.connection === 'auto') await relink();
});
