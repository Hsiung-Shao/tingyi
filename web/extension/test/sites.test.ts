import { afterEach, describe, expect, it, vi } from 'vitest';
import { isYouTube, matchSite, siteKey, startPlan, updateSiteRule, type SiteRule } from '../src/shared/sites.ts';
import { DEFAULT_SETTINGS, loadSettings, SETTINGS_KEY } from '../src/shared/settings.ts';

afterEach(() => vi.unstubAllGlobals());

/** 只假造 chrome.storage.local(外部 API) */
function stubStorage(store: Record<string, unknown>) {
  vi.stubGlobal('chrome', {
    storage: { local: { get: async (key: string) => (key in store ? { [key]: structuredClone(store[key]) } : {}) } },
  });
}

describe('網站規則', () => {
  // U8
  it('舊擴充設定沒有網站規則時 sites 為空物件', async () => {
    const { sites: _omit, ...old } = DEFAULT_SETTINGS;
    stubStorage({ [SETTINGS_KEY]: old });
    const s = await loadSettings();
    expect(s.sites).toEqual({}); // 沒補預設會是 undefined,後面 matchSite 就炸
    expect(DEFAULT_SETTINGS.sites).toEqual({});
  });

  // T10
  it('舊擴充設定沒有簡繁轉換欄位時預設 terms', async () => {
    const old: Record<string, unknown> = { ...DEFAULT_SETTINGS };
    delete old.zhConvert;
    stubStorage({ [SETTINGS_KEY]: old });
    const s = await loadSettings();
    expect(s.zhConvert).toBe('terms'); // 沒補預設會是 undefined,轉換函式拿到未知模式
    expect(DEFAULT_SETTINGS.zhConvert).toBe('terms');
  });

  // U9
  it('網站規則比對含子網域且不誤中相似網域', () => {
    const rule: SiteRule = { auto: true };
    const sites = { 'reddit.com': rule };
    for (const host of ['reddit.com', 'www.reddit.com', 'old.reddit.com']) {
      const hit = matchSite(host, sites);
      expect(hit?.key, host).toBe('reddit.com');
      expect(hit?.rule).toEqual(rule);
    }
    expect(matchSite('notreddit.com', sites)).toBeUndefined(); // 只比字尾會誤中
    expect(matchSite('reddit.com.evil.io', sites)).toBeUndefined(); // 只比前綴會誤中
  });

  it('多個網站規則同時命中時取最長的網域', () => {
    const sites = { 'x.com': { auto: false }, 'sub.x.com': { auto: true } };
    expect(matchSite('a.sub.x.com', sites)?.key).toBe('sub.x.com');
    expect(matchSite('b.x.com', sites)?.key).toBe('x.com');
  });

  // U10
  it('依規則決定開頁動作', () => {
    expect(startPlan('www.reddit.com', undefined)).toEqual({ page: false, captions: false });
    expect(startPlan('www.reddit.com', { auto: true })).toEqual({ page: true, captions: false });
    expect(startPlan('www.reddit.com', { auto: false })).toEqual({ page: false, captions: false });
    expect(startPlan('www.youtube.com', { auto: false, captions: true })).toEqual({ page: false, captions: true });
    expect(startPlan('www.youtube.com', { auto: true, captions: true })).toEqual({ page: true, captions: true });
    expect(startPlan('www.youtube.com', { auto: true })).toEqual({ page: true, captions: false });
  });

  it('非 YouTube 網站的字幕設定一律無效', () => {
    expect(startPlan('www.reddit.com', { auto: true, captions: true })).toEqual({ page: true, captions: false });
    expect(startPlan('notyoutube.com', { auto: false, captions: true }).captions).toBe(false);
  });

  // U9b
  it('popup 網站鍵依規則、YouTube、一般網域決定', () => {
    expect(siteKey('www.reddit.com', {})).toBe('reddit.com'); // 沒去掉 www. 會多出一條重複規則
    expect(siteKey('old.reddit.com', { 'reddit.com': { auto: true } })).toBe('reddit.com'); // 已有規則要沿用
    expect(siteKey('m.youtube.com', {})).toBe('youtube.com');
    expect(siteKey('x.com', {})).toBe('x.com');
  });

  // U10b
  it('更新網站規則:新增、全關即刪除、保留字幕設定、不改原物件', () => {
    const empty: Record<string, SiteRule> = {};
    const added = updateSiteRule(empty, 'reddit.com', { auto: true });
    expect(added).toEqual({ 'reddit.com': { auto: true } });
    expect(empty).toEqual({}); // 直接改原物件會讓 UI 狀態比對失效

    const removed = updateSiteRule(added, 'reddit.com', { auto: false });
    expect(removed).toEqual({}); // 全關還留著會讓設定越積越多
    expect(added).toEqual({ 'reddit.com': { auto: true } });

    const yt = updateSiteRule({}, 'youtube.com', { captions: true });
    expect(yt['youtube.com']?.captions).toBe(true); // 只開字幕時 auto 為 false 也不能被刪
    expect(yt['youtube.com']?.auto).toBe(false);
  });

  it('YouTube 判斷含子網域、不誤中相似網域', () => {
    expect(isYouTube('youtube.com')).toBe(true);
    expect(isYouTube('www.youtube.com')).toBe(true);
    expect(isYouTube('m.youtube.com')).toBe(true);
    expect(isYouTube('notyoutube.com')).toBe(false);
    expect(isYouTube('youtube.com.evil.io')).toBe(false);
  });
});
