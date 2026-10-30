// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createBall, type BallHandle } from '../src/content/ball.ts';

let ball: BallHandle | undefined;
afterEach(() => {
  ball?.destroy();
  ball = undefined;
});

describe('浮動球只接受使用者本人的操作', () => {
  it('網頁程式用 click() 代按時不會開關翻譯或改設定', () => {
    const cbs = { onToggle: vi.fn(), onMode: vi.fn(), onScope: vi.fn(), onOpenOptions: vi.fn() };
    // 不帶 allowSyntheticEvents:等同正式環境
    ball = createBall(document, { ...cbs, initial: { state: { enabled: false, done: 0, pending: 0, errors: 0 }, prefs: { displayMode: 'dual', scope: 'main' } } });
    const q = (n: string) => ball!.host.shadowRoot!.querySelector<HTMLElement>(`[data-atx="${n}"]`)!;
    q('toggle').click();
    q('panel-toggle').click();
    q('mode-only').click();
    q('scope-page').click();
    q('options').click();
    expect(cbs.onToggle).not.toHaveBeenCalled();
    expect(cbs.onMode).not.toHaveBeenCalled();
    expect(cbs.onScope).not.toHaveBeenCalled();
    expect(cbs.onOpenOptions).not.toHaveBeenCalled();
  });
});
