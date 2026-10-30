// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createBall, type BallHandle, type BallPrefs } from '../src/content/ball.ts';
import { collectBlocks, getBlockText } from '../src/content/dom.ts';
import type { TabState } from '../src/shared/messages.ts';

const idle: TabState = { enabled: false, done: 0, pending: 0, errors: 0 };
const prefs0: BallPrefs = { displayMode: 'dual', scope: 'main' };

let ball: BallHandle | undefined;

beforeEach(() => {
  document.body.innerHTML = '<p>Page paragraph text</p>';
});
afterEach(() => {
  ball?.destroy();
  ball = undefined;
  document.body.innerHTML = '';
});

function q(h: BallHandle, name: string): HTMLElement {
  const el = h.host.shadowRoot?.querySelector<HTMLElement>(`[data-atx="${name}"]`);
  if (!el) throw new Error(`找不到 [data-atx="${name}"]`);
  return el;
}

function make(over: Partial<Parameters<typeof createBall>[1]> = {}) {
  const cbs = {
    onToggle: vi.fn(),
    onMode: vi.fn(),
    onScope: vi.fn(),
    onOpenOptions: vi.fn(),
    onMove: vi.fn(),
  };
  // jsdom 派發的事件 isTrusted 為 false,測試要開啟才能模擬使用者操作
  ball = createBall(document, { ...cbs, initial: { state: idle, prefs: prefs0, top: 200 }, allowSyntheticEvents: true, ...over });
  return { handle: ball, cbs };
}

/** 派發 pointer 事件;jsdom 沒有 PointerEvent 時退回 MouseEvent */
function fire(el: Element, type: string, clientY: number, clientX = 10) {
  const init = { clientX, clientY, bubbles: true, composed: true, cancelable: true, button: 0 };
  const ev =
    typeof PointerEvent === 'function'
      ? new PointerEvent(type, { ...init, pointerId: 1, isPrimary: true })
      : new MouseEvent(type, init);
  el.dispatchEvent(ev);
}

describe('浮動球結構', () => {
  it('U9 host 在 documentElement 底下、有 shadowRoot、不在 body 裡', () => {
    const { handle } = make();
    expect(handle.host.parentElement).toBe(document.documentElement);
    expect(handle.host.shadowRoot).not.toBeNull();
    expect(document.body.contains(handle.host)).toBe(false);
  });

  it('U9 建立浮動球後收集到的段落與沒有時相同', () => {
    const opts = { targetLang: 'zh-TW', minChars: 2, scope: 'page' as const };
    const before = collectBlocks(document.body, opts).map(getBlockText);
    expect(before).toEqual(['Page paragraph text']);
    document.body.innerHTML = '<p>Page paragraph text</p>';
    const { handle } = make();
    expect(document.documentElement.contains(handle.host)).toBe(true);
    const after = collectBlocks(document.body, opts).map(getBlockText);
    expect(after).toEqual(before);
  });

  it('U9 destroy 後 host 從頁面移除', () => {
    const { handle } = make();
    expect(document.documentElement.contains(handle.host)).toBe(true);
    handle.destroy();
    expect(document.documentElement.contains(handle.host)).toBe(false);
  });
});

describe('浮動球互動', () => {
  it('U10 點主按鈕呼叫 onToggle 一次', () => {
    const { handle, cbs } = make();
    q(handle, 'toggle').click();
    expect(cbs.onToggle).toHaveBeenCalledTimes(1);
    expect(cbs.onMode).not.toHaveBeenCalled();
  });

  it('U11 面板點「僅譯文」「整頁」呼叫對應回呼並更新 aria-pressed', () => {
    let prefs = prefs0;
    const modeSpy = vi.fn();
    const scopeSpy = vi.fn();
    // 模擬呼叫端:收到回呼後呼叫 update(新 prefs)
    const { handle } = make({
      onMode: (m) => {
        modeSpy(m);
        prefs = { ...prefs, displayMode: m };
        ball?.update(idle, prefs);
      },
      onScope: (s) => {
        scopeSpy(s);
        prefs = { ...prefs, scope: s };
        ball?.update(idle, prefs);
      },
    });
    q(handle, 'expand').click();
    expect(q(handle, 'mode-dual').getAttribute('aria-pressed')).toBe('true');
    expect(q(handle, 'mode-only').getAttribute('aria-pressed')).toBe('false');
    expect(q(handle, 'scope-main').getAttribute('aria-pressed')).toBe('true');
    expect(q(handle, 'scope-page').getAttribute('aria-pressed')).toBe('false');

    q(handle, 'mode-only').click();
    q(handle, 'scope-page').click();
    expect(modeSpy).toHaveBeenCalledWith('only');
    expect(scopeSpy).toHaveBeenCalledWith('page');
    expect(q(handle, 'mode-only').getAttribute('aria-pressed')).toBe('true');
    expect(q(handle, 'mode-dual').getAttribute('aria-pressed')).toBe('false');
    expect(q(handle, 'scope-page').getAttribute('aria-pressed')).toBe('true');
    expect(q(handle, 'scope-main').getAttribute('aria-pressed')).toBe('false');
  });

  it('U11 點進階設定呼叫 onOpenOptions', () => {
    const { handle, cbs } = make();
    q(handle, 'expand').click();
    q(handle, 'options').click();
    expect(cbs.onOpenOptions).toHaveBeenCalledTimes(1);
  });

  it('U12 update 反映啟用狀態與已翻段數', () => {
    const { handle } = make();
    expect(q(handle, 'toggle').getAttribute('aria-pressed')).toBe('false');
    handle.update({ enabled: true, done: 3, pending: 0, errors: 0 }, prefs0);
    expect(q(handle, 'toggle').getAttribute('aria-pressed')).toBe('true');
    expect(q(handle, 'status').textContent).toContain('3');
  });

  it('U12 update 關閉後 aria-pressed 回到 false', () => {
    const { handle } = make();
    handle.update({ enabled: true, done: 3, pending: 0, errors: 0 }, prefs0);
    handle.update({ enabled: false, done: 0, pending: 0, errors: 0 }, prefs0);
    expect(q(handle, 'toggle').getAttribute('aria-pressed')).toBe('false');
  });

  it('U13 主按鈕拖曳超過 4px 不觸發 onToggle 且 top 改變', () => {
    const { handle, cbs } = make();
    const toggle = q(handle, 'toggle');
    const topBefore = handle.host.style.top;
    fire(toggle, 'pointerdown', 100);
    fire(toggle, 'pointermove', 130);
    fire(toggle, 'pointerup', 130);
    toggle.dispatchEvent(new MouseEvent('click', { bubbles: true, composed: true, clientY: 130 }));
    expect(cbs.onToggle).not.toHaveBeenCalled();
    expect(handle.host.style.top).not.toBe(topBefore);
    expect(cbs.onMove).toHaveBeenCalledTimes(1);
  });
});
