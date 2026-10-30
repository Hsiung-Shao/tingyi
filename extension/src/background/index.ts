import {
  buildPrompt,
  callOpenAICompatible,
  translateWithRetry,
  withCache,
  type KeyValueStore,
  type PageContext,
  type Segment,
} from '@atx/core';
import type { ContentRequest, LimitsResponse, PopupRequest, ProvidersResponse, TranslateResponse } from '../shared/messages.ts';
import { effectiveLimits, loadSettings, type Settings } from '../shared/settings.ts';
import { callBridgeTool } from './mcp.ts';

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

/** 簡單的併發上限,避免同時開太多 CLI 或打爆本地模型 */
let running = 0;
const waiters: (() => void)[] = [];
async function withSlot<T>(limit: number, fn: () => Promise<T>): Promise<T> {
  while (running >= limit) await new Promise<void>((r) => waiters.push(r));
  running++;
  try {
    return await fn();
  } finally {
    running--;
    waiters.shift()?.();
  }
}

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

async function translateDirect(s: Settings, segments: Segment[], context: PageContext): Promise<Map<number, string>> {
  const extraBody = JSON.parse(s.direct.extraBody || '{}') as Record<string, unknown>;
  const cfg = {
    baseUrl: s.direct.baseUrl,
    model: s.direct.model,
    apiKey: s.direct.apiKey || undefined,
    jsonMode: s.direct.format === 'json' && s.direct.jsonMode,
    extraBody,
  };
  return translateWithRetry(segments, (segs) =>
    callOpenAICompatible(cfg, buildPrompt(segs, s.targetLang, s.direct.format, context)),
  );
}

async function handleTranslate(segments: Segment[], context: PageContext): Promise<TranslateResponse> {
  const s = await loadSettings();
  const scope =
    s.route === 'bridge'
      ? { provider: `bridge:${s.bridge.provider}`, model: s.bridge.model, targetLang: s.targetLang }
      : { provider: `direct:${s.direct.baseUrl}`, model: s.direct.model, targetLang: s.targetLang };
  const translate = withCache(store, scope, (segs) =>
    withSlot(s.concurrency, () => (s.route === 'bridge' ? translateViaBridge(s, segs, context) : translateDirect(s, segs, context))),
  );
  const map = await translate(segments);
  return { ok: true, translations: [...map].map(([id, text]) => ({ id, text })) };
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
    handleTranslate(msg.segments, msg.context).then(sendResponse, fail);
    return true;
  }
  if (msg.type === 'getLimits') {
    loadSettings().then((s) => sendResponse({ ...effectiveLimits(s), targetLang: s.targetLang } satisfies LimitsResponse), fail);
    return true;
  }
  if (msg.type === 'listProviders') {
    handleListProviders().then(sendResponse, fail);
    return true;
  }
  return false;
});

chrome.commands.onCommand.addListener((command, tab) => {
  if (command === 'toggle-translate' && tab?.id !== undefined) {
    chrome.tabs.sendMessage(tab.id, { type: 'toggle' }).catch(() => {});
  }
});
