import { splitBatches, type Segment } from '@atx/core';
import type { LimitsResponse, TabRequest, TabState, TranslateResponse } from '../shared/messages.ts';
import { collectBlocks, getBlockText, insertTranslation, removeAllTranslations, watchNewBlocks, type CollectOptions } from './dom.ts';

// esbuild 打包成 IIFE,頂層宣告不會跟其他 content script 撞名
let enabled = false;
let limits: LimitsResponse | null = null;
let nextId = 1;
let io: IntersectionObserver | null = null;
let stopWatch: (() => void) | null = null;
let flushTimer: ReturnType<typeof setTimeout> | undefined;
let generation = 0;
const queue = new Map<number, HTMLElement>();
const state: TabState = { enabled: false, done: 0, pending: 0, errors: 0 };

function observe(blocks: HTMLElement[]): void {
  for (const el of blocks) {
    el.dataset.atxState = 'seen';
    io?.observe(el);
  }
}

function enqueue(el: HTMLElement): void {
  if (el.dataset.atxState !== 'seen') return;
  el.dataset.atxState = 'pending';
  queue.set(nextId++, el);
  state.pending++;
  clearTimeout(flushTimer);
  flushTimer = setTimeout(flush, 300);
}

function flush(): void {
  if (!limits || queue.size === 0) return;
  const segments: Segment[] = [];
  const owners = new Map<number, HTMLElement>();
  for (const [id, el] of queue) {
    segments.push({ id, text: getBlockText(el) });
    owners.set(id, el);
  }
  queue.clear();
  const gen = generation;
  for (const batch of splitBatches(segments, limits)) void send(batch, owners, gen);
}

async function send(batch: Segment[], owners: Map<number, HTMLElement>, gen: number): Promise<void> {
  let res: TranslateResponse;
  try {
    res = await chrome.runtime.sendMessage({ type: 'translate', segments: batch, context: { title: document.title, url: location.href } });
  } catch (e) {
    res = { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  // 翻譯途中被關閉(或關了又開)就丟掉這批結果
  if (gen !== generation) return;
  const got = new Map(res.ok ? res.translations.map((t) => [t.id, t.text]) : []);
  for (const seg of batch) {
    const el = owners.get(seg.id);
    if (!el) continue;
    state.pending--;
    const text = got.get(seg.id);
    if (text !== undefined) {
      insertTranslation(el, text);
      el.dataset.atxState = 'done';
      state.done++;
    } else {
      el.dataset.atxState = 'error';
      el.dataset.atxError = res.ok ? '這段沒有拿到譯文' : res.error;
      state.errors++;
      state.lastError = res.ok ? '部分段落沒有拿到譯文' : res.error;
    }
  }
}

async function enable(): Promise<void> {
  enabled = state.enabled = true;
  const gen = ++generation;
  const got = (await chrome.runtime.sendMessage({ type: 'getLimits' })) as LimitsResponse;
  // 等設定的期間被關掉(或關了又開)就不再建立觀察
  if (!enabled || gen !== generation) return;
  limits = got;
  io?.disconnect();
  stopWatch?.();
  const opts: CollectOptions = { targetLang: limits.targetLang, minChars: 2 };
  io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        io?.unobserve(e.target);
        enqueue(e.target as HTMLElement);
      }
    },
    { rootMargin: '0px 0px 200% 0px' },
  );
  observe(collectBlocks(document.body, opts));
  stopWatch = watchNewBlocks(document.body, opts, observe);
}

function disable(): void {
  enabled = state.enabled = false;
  generation++;
  io?.disconnect();
  io = null;
  stopWatch?.();
  stopWatch = null;
  clearTimeout(flushTimer);
  queue.clear();
  removeAllTranslations(document.body);
  Object.assign(state, { done: 0, pending: 0, errors: 0, lastError: undefined });
}

chrome.runtime.onMessage.addListener((msg: TabRequest, _sender, sendResponse) => {
  if (msg.type === 'toggle') {
    if (enabled) {
      disable();
      sendResponse(state);
    } else {
      enable().then(
        () => sendResponse(state),
        (e) => {
          state.lastError = String(e);
          sendResponse(state);
        },
      );
      return true;
    }
  } else if (msg.type === 'getState') {
    sendResponse(state);
  }
  return false;
});
