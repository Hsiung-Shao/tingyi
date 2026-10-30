// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';

type Listener = (msg: unknown, sender: unknown, sendResponse: (r: unknown) => void) => boolean | void;

describe('content 錯誤顯示', () => {
  it('翻譯失敗時錯誤細節只留在擴充狀態,不寫進網頁 DOM', async () => {
    document.body.innerHTML = '<p>Paragraph that will fail to translate</p>';
    let listener: Listener | undefined;
    const SECRET = 'C:\\Users\\someone\\.codex\\auth.json not logged in';
    vi.stubGlobal('chrome', {
      runtime: {
        onMessage: { addListener: (l: Listener) => (listener = l) },
        sendMessage: async (msg: { type: string }) => {
          if (msg.type === 'getLimits') return { maxSegments: 20, maxChars: 6000, targetLang: 'zh-TW' };
          return { ok: false, error: SECRET };
        },
      },
    });
    // 假 IntersectionObserver:一 observe 就回報可見
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        cb: (e: { isIntersecting: boolean; target: Element }[]) => void;
        constructor(cb: (e: { isIntersecting: boolean; target: Element }[]) => void) {
          this.cb = cb;
        }
        observe(el: Element) {
          this.cb([{ isIntersecting: true, target: el }]);
        }
        unobserve() {}
        disconnect() {}
      },
    );
    await import('../src/content/index.ts');
    const send = (type: string) => new Promise((r) => listener!({ type }, {}, r));
    await send('toggle');
    await new Promise((r) => setTimeout(r, 400)); // 等 300ms 的批次 flush 與回應
    const state = (await send('getState')) as { errors: number; lastError?: string };

    expect(state.errors).toBe(1);
    expect(state.lastError).toContain('not logged in');
    expect(document.querySelector('[data-atx-state="error"]')).not.toBeNull();
    expect(document.documentElement.outerHTML).not.toContain('auth.json');
    vi.unstubAllGlobals();
  });
});
