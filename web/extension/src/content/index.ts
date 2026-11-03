import { splitBatches, type Segment } from '@atx/core';
import type { LimitsResponse, TabRequest, TabState, TranslateResponse } from '../shared/messages.ts';
import { SETTINGS_KEY, type DisplayMode, type Scope, type Settings } from '../shared/settings.ts';
import { createBall, type BallHandle, type BallPrefs } from './ball.ts';
import {
  collectBlocks,
  getBlockText,
  insertTranslation,
  removeAllTranslations,
  resetForRetranslate,
  setDisplayMode,
  watchNewBlocks,
  type CollectOptions,
} from './dom.ts';
import { isYouTube, matchSite, startPlan } from '../shared/sites.ts';

const BALL_TOP_KEY = 'atx:ballTop';

// esbuild 打包成 IIFE,頂層宣告不會跟其他 content script 撞名
let ball: BallHandle | null = null;
const prefs: BallPrefs = { displayMode: 'dual', scope: 'main' };
let nextId = 1;

/**
 * 一組翻譯:頁面翻譯(浮動球、popup、Alt+A)與 YouTube 字幕翻譯各一組,各自開關、各自監看。
 * captions:YouTube 上頁面翻譯不收字幕框(exclude),字幕翻譯只收字幕框(only);其他網站不篩選。
 */
function createSession(captions: CollectOptions['captions'], onChange: () => void) {
  let enabled = false;
  let limits: LimitsResponse | null = null;
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
    onChange();
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
    await acquire();
    let res: TranslateResponse;
    try {
      // 排隊期間被關閉就不送了
      if (gen !== generation) return;
      res = await chrome.runtime.sendMessage({ type: 'translate', segments: batch, context: { title: document.title, url: location.href } });
    } catch (e) {
      res = { ok: false, error: e instanceof Error ? e.message : String(e) };
    } finally {
      release();
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
        const stale = insertTranslation(el, text, seg.text);
        el.dataset.atxState = 'done';
        state.done++;
        // 翻譯途中頁面改了這段文字:重新排入(有次數上限)
        if (stale && resetForRetranslate(el)) observe([el]);
      } else {
        // 錯誤細節可能含本機路徑與 CLI 狀態,只留在擴充狀態(popup 顯示),不寫進不可信的頁面 DOM
        el.dataset.atxState = 'error';
        state.errors++;
        state.lastError = res.error ?? '部分段落沒有拿到譯文';
      }
    }
    onChange();
  }

  async function enable(): Promise<void> {
    enabled = state.enabled = true;
    const gen = ++generation;
    const got = (await chrome.runtime.sendMessage({ type: 'getLimits' })) as LimitsResponse;
    // 等設定的期間被關掉(或關了又開)就不再建立觀察
    if (!enabled || gen !== generation) return;
    limits = got;
    concurrency = got.concurrency ?? 2;
    io?.disconnect();
    stopWatch?.();
    const opts: CollectOptions = { targetLang: limits.targetLang, minChars: 2, scope: prefs.scope, captions };
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
    onChange();
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
    removeAllTranslations(document.body, captions);
    Object.assign(state, { done: 0, pending: 0, errors: 0, lastError: undefined });
    onChange();
  }

  async function toggle(): Promise<TabState> {
    if (enabled) disable();
    else {
      try {
        await enable();
      } catch (e) {
        state.lastError = String(e);
        onChange();
      }
    }
    return state;
  }

  return {
    state,
    toggle,
    disable,
    get enabled() {
      return enabled;
    },
  };
}

// 併發在這裡控制、拿到名額才送:若讓背景排隊,排隊時間會算進 SW 單一事件 5 分鐘上限;頁面與字幕共用名額
let concurrency = 2;
let inFlight = 0;
const waiting: (() => void)[] = [];
async function acquire(): Promise<void> {
  while (inFlight >= concurrency) await new Promise<void>((r) => waiting.push(r));
  inFlight++;
}
function release(): void {
  inFlight--;
  waiting.shift()?.();
}

const youtube = isYouTube(location.hostname);
const page = createSession(youtube ? 'exclude' : undefined, () => refreshBall());
/** YouTube 字幕翻譯,只看網站規則的「翻譯影片字幕」 */
const captionSession = youtube ? createSession('only', () => {}) : null;
const state = page.state;

/** Alt+A、popup、浮動球共用的開關 */
async function toggle(): Promise<TabState> {
  return page.toggle();
}

/** 依網站規則自動開啟;規則改了就即時套用(取消自動不會關掉已開的頁面翻譯) */
function applySiteRule(sites: Settings['sites'] | undefined): void {
  const plan = startPlan(location.hostname, matchSite(location.hostname, sites ?? {})?.rule);
  if (plan.page && !page.enabled) void page.toggle();
  if (captionSession && plan.captions !== captionSession.enabled) void captionSession.toggle();
}

function refreshBall(): void {
  ball?.update({ ...state }, { ...prefs });
}

function applyPrefs(next: Partial<Settings> | undefined): void {
  const mode: DisplayMode = next?.displayMode ?? 'dual';
  const scope: Scope = next?.scope ?? 'main';
  const scopeChanged = scope !== prefs.scope;
  prefs.displayMode = mode;
  prefs.scope = scope;
  setDisplayMode(mode);
  // 範圍改了就重來一次;已翻過的段落會命中快取
  if (scopeChanged && page.enabled) {
    page.disable();
    void page.toggle();
  }
  refreshBall();
}

/** 浮動球的設定寫回 storage;各分頁(含這一頁)由 storage.onChanged 套用 */
async function savePref(patch: Partial<Pick<Settings, 'displayMode' | 'scope'>>): Promise<void> {
  const raw = (await chrome.storage.local.get(SETTINGS_KEY))[SETTINGS_KEY] as Partial<Settings> | undefined;
  await chrome.storage.local.set({ [SETTINGS_KEY]: { ...raw, ...patch } });
}

async function initBall(): Promise<void> {
  const store = chrome.storage?.local;
  let top: number | undefined;
  if (store) {
    const got = await store.get([SETTINGS_KEY, BALL_TOP_KEY]);
    applyPrefs(got[SETTINGS_KEY] as Partial<Settings> | undefined);
    applySiteRule((got[SETTINGS_KEY] as Partial<Settings> | undefined)?.sites);
    top = typeof got[BALL_TOP_KEY] === 'number' ? (got[BALL_TOP_KEY] as number) : undefined;
    chrome.storage.onChanged.addListener((changes, area) => {
      if (area !== 'local' || !changes[SETTINGS_KEY]) return;
      const next = changes[SETTINGS_KEY].newValue as Partial<Settings> | undefined;
      applyPrefs(next);
      applySiteRule(next?.sites);
    });
  }
  ball = createBall(document, {
    onToggle: () => void toggle(),
    onMode: (displayMode) => void savePref({ displayMode }),
    onScope: (scope) => void savePref({ scope }),
    onOpenOptions: () => void chrome.runtime.sendMessage({ type: 'openOptions' }),
    onMove: (t) => void store?.set({ [BALL_TOP_KEY]: t }),
    initial: { state: { ...state }, prefs: { ...prefs }, top },
  });
}

chrome.runtime.onMessage.addListener((msg: TabRequest, _sender, sendResponse) => {
  if (msg.type === 'toggle') {
    void toggle().then(sendResponse);
    return true;
  } else if (msg.type === 'getState') {
    sendResponse(state);
  }
  return false;
});

// 只在最上層頁面顯示浮動球(iframe 裡不重複出現)
if (window.top === window) void initBall();
