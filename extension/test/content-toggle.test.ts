// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';

type Listener = (msg: unknown, sender: unknown, sendResponse: (r: unknown) => void) => boolean | void;

describe('content 開關', () => {
  it('開啟途中又關閉時不會繼續建立觀察與送翻', async () => {
    document.body.innerHTML = '<p>Paragraph that should not be translated</p>';
    // 只假造瀏覽器外部 API:chrome.runtime 與 IntersectionObserver
    let listener: Listener | undefined;
    let releaseLimits: (v: unknown) => void = () => {};
    const sent: unknown[] = [];
    const observed: Element[] = [];
    vi.stubGlobal('chrome', {
      runtime: {
        onMessage: { addListener: (l: Listener) => (listener = l) },
        sendMessage: (msg: { type: string }) => {
          sent.push(msg);
          if (msg.type === 'getLimits') return new Promise((r) => (releaseLimits = r));
          return Promise.resolve({ ok: true, translations: [] });
        },
      },
    });
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        observe(el: Element) {
          observed.push(el);
        }
        unobserve() {}
        disconnect() {}
      },
    );
    await import('../src/content/index.ts');
    const toggle = () => new Promise((r) => listener!({ type: 'toggle' }, {}, r));

    void toggle(); // 開:卡在 getLimits
    const off = (await toggle()) as { enabled: boolean }; // 立刻關
    releaseLimits({ maxSegments: 20, maxChars: 6000, targetLang: 'zh-TW' });
    await new Promise((r) => setTimeout(r, 50));

    expect(off.enabled).toBe(false);
    expect(observed).toEqual([]);
    expect(document.querySelector('[data-atx-state]')).toBeNull();
    vi.unstubAllGlobals();
  });
});
