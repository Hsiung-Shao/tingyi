// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';

type Listener = (msg: unknown, sender: unknown, sendResponse: (r: unknown) => void) => boolean | void;

describe('content 併發', () => {
  it('同時送給背景的翻譯請求不超過設定的併發數', async () => {
    document.body.innerHTML = Array.from({ length: 8 }, (_, i) => `<p>Paragraph number ${i} needs translating</p>`).join('');
    let listener: Listener | undefined;
    let inFlight = 0;
    let peak = 0;
    let total = 0;
    vi.stubGlobal('chrome', {
      runtime: {
        onMessage: { addListener: (l: Listener) => (listener = l) },
        sendMessage: async (msg: { type: string; segments?: { id: number }[] }) => {
          if (msg.type === 'getLimits') return { maxSegments: 1, maxChars: 6000, targetLang: 'zh-TW', concurrency: 2 };
          inFlight++;
          total++;
          peak = Math.max(peak, inFlight);
          await new Promise((r) => setTimeout(r, 50));
          inFlight--;
          return { ok: true, translations: msg.segments!.map((s) => ({ id: s.id, text: '譯' })) };
        },
      },
    });
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
    await new Promise((r) => listener!({ type: 'toggle' }, {}, r));
    await vi.waitFor(() => expect(document.querySelectorAll('font.atx-target')).toHaveLength(8), { timeout: 3000 });

    expect(total).toBe(8);
    expect(peak).toBe(2);
    vi.unstubAllGlobals();
  });
});
