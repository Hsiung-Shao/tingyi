// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';

type Listener = (msg: unknown, sender: unknown, sendResponse: (r: unknown) => void) => boolean | void;
type Changed = (changes: Record<string, { newValue: unknown }>, area: string) => void;

describe('content 網站規則', () => {
  // R3
  it('自動翻譯的網站被手動關掉後,其他設定變動不會又自動打開', async () => {
    document.body.innerHTML = '<p>Paragraph on an auto translated site.</p>';
    const settings = { displayMode: 'dual', scope: 'main', sites: { localhost: { auto: true } } };
    let listener: Listener | undefined;
    let changed: Changed | undefined;
    // 只假造瀏覽器外部 API:chrome.runtime、chrome.storage 與 IntersectionObserver
    vi.stubGlobal('chrome', {
      runtime: {
        onMessage: { addListener: (l: Listener) => (listener = l) },
        sendMessage: async (msg: { type: string }) =>
          msg.type === 'getLimits' ? { maxSegments: 8, maxChars: 2500, targetLang: 'zh-TW', concurrency: 2 } : { ok: true, translations: [] },
      },
      storage: {
        local: { get: async () => ({ 'atx:settings': settings }), set: async () => {} },
        onChanged: { addListener: (l: Changed) => (changed = l) },
      },
    });
    vi.stubGlobal('IntersectionObserver', class {
      observe() {}
      unobserve() {}
      disconnect() {}
    });
    await import('../src/content/index.ts');
    const getState = () => new Promise<{ enabled: boolean }>((r) => listener!({ type: 'getState' }, {}, r as (x: unknown) => void));
    await new Promise((r) => setTimeout(r, 50));
    expect((await getState()).enabled).toBe(true);

    const off = await new Promise<{ enabled: boolean }>((r) => listener!({ type: 'toggle' }, {}, r as (x: unknown) => void));
    expect(off.enabled).toBe(false);
    changed!({ 'atx:settings': { newValue: { ...settings, displayMode: 'only' } } }, 'local');
    await new Promise((r) => setTimeout(r, 50));
    expect((await getState()).enabled).toBe(false);
    vi.unstubAllGlobals();
  });
});
