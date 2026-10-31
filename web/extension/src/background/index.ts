import {
  buildPrompt,
  callOpenAICompatible,
  PartialTranslationError,
  translateWithRetry,
  withCache,
  type KeyValueStore,
  type PageContext,
  type Segment,
} from '@atx/core';
import type { ContentRequest, LimitsResponse, PopupRequest, ProvidersResponse, TranslateResponse } from '../shared/messages.ts';
import { effectiveLimits, loadSettings, type Settings } from '../shared/settings.ts';
import { autoConnect } from './appLink.ts';
import { callBridgeTool } from './mcp.ts';
import { createSlotPicker, withSlot } from './slots.ts';

const store: KeyValueStore = {
  async get(keys) {
    return (await chrome.storage.local.get(keys)) as Record<string, string>;
  },
  async set(entries) {
    try {
      await chrome.storage.local.set(entries);
    } catch (e) {
      // 快取寫入失敗不影響翻譯結果
      console.warn('[atx] cache write failed', e);
    }
  },
};

async function translateViaBridge(s: Settings, segments: Segment[], context: PageContext): Promise<Map<number, string>> {
  const res = await callBridgeTool<{ translations: { id: number; text: string }[] }>(s.bridge.url, s.bridge.token, 'translate_batch', {
    provider: s.bridge.provider,
    ...(s.bridge.model ? { model: s.bridge.model } : {}),
    targetLang: s.targetLang,
    segments,
    context,
  });
  return new Map(res.translations.map((t) => [t.id, t.text]));
}

// 跨請求輪用聽譯指派的 slot;slot 清單變了才換新的選擇器
let picker = { key: '', next: createSlotPicker([]) };
function nextSlot(s: Settings): number | undefined {
  // 手動設定的端點不是聽譯,不帶 id_slot
  if (s.connection !== 'auto') return undefined;
  const key = s.direct.slots.join(',');
  if (key !== picker.key) picker = { key, next: createSlotPicker(s.direct.slots) };
  return picker.next();
}

async function translateDirect(s: Settings, segments: Segment[], context: PageContext): Promise<Map<number, string>> {
  const extraBody = JSON.parse(s.direct.extraBody || '{}') as Record<string, unknown>;
  const cfg = {
    baseUrl: s.direct.baseUrl,
    model: s.direct.model,
    apiKey: s.direct.apiKey || undefined,
    jsonMode: s.direct.format === 'json' && s.direct.jsonMode,
    extraBody,
  };
  return translateWithRetry(
    segments,
    (segs) =>
      callOpenAICompatible(
        { ...cfg, extraBody: withSlot(extraBody, nextSlot(s)) },
        buildPrompt(segs, s.targetLang, s.direct.format, context),
      ),
    1,
    s.direct.format,
  );
}

async function handleTranslate(segments: Segment[], context: PageContext): Promise<TranslateResponse> {
  const s = await loadSettings();
  const scope =
    s.route === 'bridge'
      ? { provider: `bridge:${s.bridge.provider}`, model: s.bridge.model, targetLang: s.targetLang }
      : // 本機程式每次啟動的 port 不同,自動連線時快取不綁 port
        { provider: s.connection === 'auto' ? 'direct:local-app' : `direct:${s.direct.baseUrl}`, model: s.direct.model, targetLang: s.targetLang };
  // 併發由 content 端控制(拿到名額才送訊息),這裡不排隊,事件時間只等於實際呼叫時間
  const translate = withCache(store, scope, (segs) =>
    s.route === 'bridge' ? translateViaBridge(s, segs, context) : translateDirect(s, segs, context),
  );
  const toList = (m: Map<number, string>) => [...m].map(([id, text]) => ({ id, text }));
  try {
    return { ok: true, translations: toList(await translate(segments)) };
  } catch (e) {
    if (e instanceof PartialTranslationError) return { ok: true, translations: toList(e.partial), error: e.message };
    throw e;
  }
}

/** 連不上端點(本機程式重啟換了 port、金鑰換了)時,自動向本機程式重新取得端點再試一次 */
async function translateWithRelink(segments: Segment[], context: PageContext): Promise<TranslateResponse> {
  const s = await loadSettings();
  const retry = async (e: unknown): Promise<TranslateResponse> => {
    if (s.connection !== 'auto' || !isConnectionError(e)) throw e;
    let link = await autoConnect();
    // 本機程式剛被喚起時模型還在載入,最多等 60 秒
    for (let i = 0; i < 30 && link.state === 'connected' && link.status === 'starting'; i++) {
      await new Promise((r) => setTimeout(r, 2000));
      link = await autoConnect();
    }
    if (link.state !== 'connected' || link.status !== 'ready') throw new Error(linkMessage(link) ?? String(e));
    return handleTranslate(segments, context);
  };
  return handleTranslate(segments, context).catch(retry);
}

/** 網路層失敗或金鑰不符(端點換了) */
function isConnectionError(e: unknown): boolean {
  const msg = e instanceof Error ? e.message : String(e);
  return e instanceof TypeError || /Failed to fetch|ECONNREFUSED|端點回應 401|fetch failed/i.test(msg);
}

function linkMessage(link: Awaited<ReturnType<typeof autoConnect>>): string | undefined {
  if (link.state === 'not-installed') return '找不到本機程式,請先安裝 agent-translator 本機程式';
  if (link.state === 'app-error') return `本機程式沒有回應:${link.message}`;
  if (link.state === 'connected' && link.status !== 'ready') return link.message ?? `本地模型尚未就緒(${link.status})`;
  return undefined;
}

async function handleListProviders(): Promise<ProvidersResponse> {
  const s = await loadSettings();
  const res = await callBridgeTool<{ providers: Extract<ProvidersResponse, { ok: true }>['providers'] }>(
    s.bridge.url,
    s.bridge.token,
    'list_providers',
    {},
  );
  return { ok: true, providers: res.providers };
}

chrome.runtime.onMessage.addListener((msg: ContentRequest | PopupRequest, _sender, sendResponse) => {
  const fail = (e: unknown) => sendResponse({ ok: false, error: e instanceof Error ? e.message : String(e) });
  if (msg.type === 'translate') {
    translateWithRelink(msg.segments, msg.context).then(sendResponse, fail);
    return true;
  }
  if (msg.type === 'appLink') {
    autoConnect().then(sendResponse, fail);
    return true;
  }
  if (msg.type === 'getLimits') {
    loadSettings().then((s) => sendResponse({ ...effectiveLimits(s), targetLang: s.targetLang, concurrency: s.concurrency } satisfies LimitsResponse), fail);
    return true;
  }
  if (msg.type === 'listProviders') {
    handleListProviders().then(sendResponse, fail);
    return true;
  }
  if (msg.type === 'openOptions') {
    // content script 不能直接開設定頁,由浮動球面板轉請背景開
    void chrome.runtime.openOptionsPage();
    return false;
  }
  return false;
});

// 瀏覽器啟動、擴充安裝或更新時先連一次本機程式,第一次使用就不必開設定
chrome.runtime.onStartup.addListener(() => void autoConnect().catch(() => {}));
chrome.runtime.onInstalled.addListener(() => void autoConnect().catch(() => {}));

chrome.commands.onCommand.addListener((command, tab) => {
  if (command === 'toggle-translate' && tab?.id !== undefined) {
    chrome.tabs.sendMessage(tab.id, { type: 'toggle' }).catch(() => {});
  }
});
