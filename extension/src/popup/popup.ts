import type { ProvidersResponse, TabState } from '../shared/messages.ts';
import { loadSettings, saveSettings, type Route } from '../shared/settings.ts';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

async function activeTabId(): Promise<number | undefined> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  return tab?.id;
}

function renderState(s: TabState | undefined): void {
  const toggle = $<HTMLButtonElement>('toggle');
  const box = $('pageState');
  if (!s) {
    toggle.disabled = true;
    box.textContent = '這個頁面不能翻譯(瀏覽器內建頁或尚未重新整理)';
    return;
  }
  toggle.textContent = s.enabled ? '顯示原文' : '翻譯此頁';
  box.textContent = s.enabled ? `已翻 ${s.done} 段、翻譯中 ${s.pending} 段、失敗 ${s.errors} 段${s.lastError ? `\n${s.lastError}` : ''}` : '';
  box.className = `status ${s.errors ? 'bad' : 'muted'}`;
}

async function refreshProviders(): Promise<void> {
  const settings = await loadSettings();
  const select = $<HTMLSelectElement>('provider');
  const box = $('providerState');
  if (settings.route !== 'bridge') {
    box.textContent = `直連:${settings.direct.baseUrl}(${settings.direct.model || '未指定模型'})`;
    return;
  }
  box.textContent = '連線 bridge 中…';
  const res = (await chrome.runtime.sendMessage({ type: 'listProviders' })) as ProvidersResponse;
  if (!res.ok) {
    box.textContent = `bridge 連不上:${res.error}\n請先執行 pnpm bridge,並在進階設定貼上 token`;
    box.className = 'status bad';
    return;
  }
  select.replaceChildren(
    ...res.providers.map((p) => {
      const o = document.createElement('option');
      o.value = p.id;
      o.textContent = `${p.label}${p.available ? '' : '(不可用)'}${p.note ? ' ⚠' : ''}`;
      return o;
    }),
  );
  select.value = settings.bridge.provider;
  const cur = res.providers.find((p) => p.id === select.value);
  box.textContent = cur ? `${cur.available ? '可用' : '不可用'}${cur.version ? ` · ${cur.version}` : ''}${cur.note ? `\n${cur.note}` : ''}` : '請選擇 agent';
  box.className = `status ${cur?.available && !cur.note ? 'ok' : 'bad'}`;
}

async function init(): Promise<void> {
  const settings = await loadSettings();
  $<HTMLSelectElement>('route').value = settings.route;
  $<HTMLSelectElement>('targetLang').value = settings.targetLang;
  $('bridgeBox').hidden = settings.route !== 'bridge';

  const tabId = await activeTabId();
  const state = tabId === undefined ? undefined : await chrome.tabs.sendMessage(tabId, { type: 'getState' }).catch(() => undefined);
  renderState(state as TabState | undefined);

  $('toggle').addEventListener('click', async () => {
    if (tabId === undefined) return;
    renderState((await chrome.tabs.sendMessage(tabId, { type: 'toggle' })) as TabState);
  });
  $('route').addEventListener('change', async (e) => {
    const s = await loadSettings();
    s.route = (e.target as HTMLSelectElement).value as Route;
    await saveSettings(s);
    $('bridgeBox').hidden = s.route !== 'bridge';
    void refreshProviders();
  });
  $('provider').addEventListener('change', async (e) => {
    const s = await loadSettings();
    s.bridge.provider = (e.target as HTMLSelectElement).value;
    await saveSettings(s);
    void refreshProviders();
  });
  $('targetLang').addEventListener('change', async (e) => {
    const s = await loadSettings();
    s.targetLang = (e.target as HTMLSelectElement).value;
    await saveSettings(s);
  });
  $('openOptions').addEventListener('click', (e) => {
    e.preventDefault();
    void chrome.runtime.openOptionsPage();
  });
  // 每秒更新頁面進度
  setInterval(async () => {
    if (tabId === undefined) return;
    const s = await chrome.tabs.sendMessage(tabId, { type: 'getState' }).catch(() => undefined);
    if (s) renderState(s as TabState);
  }, 1000);
  await refreshProviders();
}

void init();
