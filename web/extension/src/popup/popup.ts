import type { LinkStatus } from '../background/appLink.ts';
import type { ProvidersResponse, TabState } from '../shared/messages.ts';
import { loadSettings, saveSettings, type DisplayMode, type Route, type Scope } from '../shared/settings.ts';
import { isYouTube, siteKey, updateSiteRule, type SiteRule } from '../shared/sites.ts';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

async function activeTab(): Promise<chrome.tabs.Tab | undefined> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  return tab;
}

/** 「這個網站」:開啟時自動翻譯;YouTube 另有字幕開關。改了寫回設定,分頁由 storage.onChanged 套用 */
async function initSite(url: string | undefined): Promise<void> {
  let host = '';
  try {
    if (url && /^https?:/.test(url)) host = new URL(url).hostname;
  } catch {
    // 網址解析不了(瀏覽器內建頁)就不顯示這一區
  }
  if (!host) return;
  const s = await loadSettings();
  const key = siteKey(host, s.sites);
  const rule = s.sites[key];
  $('siteBox').hidden = false;
  $('siteHost').textContent = key;
  $<HTMLInputElement>('siteAuto').checked = rule?.auto === true;
  $('ytBox').hidden = !isYouTube(host);
  $<HTMLInputElement>('siteCaptions').checked = rule?.captions === true;
  $<HTMLInputElement>('siteListen').checked = rule?.listen === true;
  const save = async (patch: Partial<SiteRule>) => {
    const cur = await loadSettings();
    cur.sites = updateSiteRule(cur.sites, key, patch);
    await saveSettings(cur);
  };
  $('siteAuto').addEventListener('change', (e) => void save({ auto: (e.target as HTMLInputElement).checked }));
  $('siteCaptions').addEventListener('change', (e) => void save({ captions: (e.target as HTMLInputElement).checked }));
  $('siteListen').addEventListener('change', (e) => void save({ listen: (e.target as HTMLInputElement).checked }));
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

const MODEL_STATUS: Record<string, string> = {
  ready: '本地模型執行中',
  downloading: '本機程式正在下載模型',
  starting: '本地模型啟動中',
  stopped: '本地模型已停止',
  'not-downloaded': '本機程式尚未下載模型',
  error: '本地模型發生錯誤',
};

/** 頂端一行:本機程式連線狀態(手動設定時不顯示) */
async function refreshAppLink(): Promise<void> {
  const box = $('appLink');
  const text = $('appLinkText');
  const action = $<HTMLButtonElement>('appLinkAction');
  const link = (await chrome.runtime.sendMessage({ type: 'appLink' }).catch((e: unknown) => ({
    state: 'app-error',
    message: String(e),
  }))) as LinkStatus;
  box.hidden = link.state === 'manual';
  action.hidden = true;
  if (link.state === 'connected') {
    text.textContent = link.status === 'ready' ? '✓ 已連上本機程式' : `${MODEL_STATUS[link.status] ?? link.status}${link.message ? `:${link.message}` : ''}`;
    text.className = `status ${link.status === 'ready' ? 'ok' : 'muted'}`;
    if (link.status !== 'ready') {
      // 再送一次 hello:本機程式沒開會被喚起,開了就重抓狀態
      action.textContent = '重新整理';
      action.hidden = false;
      action.onclick = () => void refreshAppLink().then(refreshProviders);
    }
  } else if (link.state === 'not-installed') {
    text.textContent = '尚未安裝本機程式,本地翻譯需要它';
    text.className = 'status bad';
    action.textContent = '安裝說明';
    action.hidden = false;
    action.onclick = () => void chrome.tabs.create({ url: chrome.runtime.getURL('options/options.html#app') });
  } else if (link.state === 'app-error') {
    text.textContent = `本機程式沒有回應:${link.message}`;
    text.className = 'status bad';
  }
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

  const tab = await activeTab();
  const tabId = tab?.id;
  const state = tabId === undefined ? undefined : await chrome.tabs.sendMessage(tabId, { type: 'getState' }).catch(() => undefined);
  renderState(state as TabState | undefined);
  await initSite(tab?.url);

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
  $<HTMLSelectElement>('displayMode').value = settings.displayMode;
  $<HTMLSelectElement>('scope').value = settings.scope;
  // 寫回設定後,各分頁的 content script 由 storage.onChanged 即時套用
  $('displayMode').addEventListener('change', async (e) => {
    const s = await loadSettings();
    s.displayMode = (e.target as HTMLSelectElement).value as DisplayMode;
    await saveSettings(s);
  });
  $('scope').addEventListener('change', async (e) => {
    const s = await loadSettings();
    s.scope = (e.target as HTMLSelectElement).value as Scope;
    await saveSettings(s);
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
  await refreshAppLink();
  await refreshProviders();
}

void init();
