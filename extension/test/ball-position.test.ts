// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { createBall, type BallHandle } from '../src/content/ball.ts';

let ball: BallHandle | undefined;
afterEach(() => {
  ball?.destroy();
  ball = undefined;
});

const make = (top: number) =>
  createBall(document, {
    onToggle() {},
    onMode() {},
    onScope() {},
    onOpenOptions() {},
    initial: { state: { enabled: false, done: 0, pending: 0, errors: 0 }, prefs: { displayMode: 'dual', scope: 'main' }, top },
  });

describe('浮動球位置', () => {
  it('存下來的位置超出目前視窗時夾回視窗內', () => {
    Object.defineProperty(window, 'innerHeight', { value: 700, configurable: true });
    ball = make(1200);
    const top = parseFloat(ball.host.style.top);
    expect(top).toBeGreaterThanOrEqual(8);
    expect(top).toBeLessThanOrEqual(700 - 48);
  });

  it('視窗縮小時重新夾回視窗內', () => {
    Object.defineProperty(window, 'innerHeight', { value: 900, configurable: true });
    ball = make(800);
    expect(parseFloat(ball.host.style.top)).toBe(800);
    Object.defineProperty(window, 'innerHeight', { value: 500, configurable: true });
    window.dispatchEvent(new Event('resize'));
    expect(parseFloat(ball.host.style.top)).toBeLessThanOrEqual(500 - 48);
  });

  it('浮動球的按鈕不在 Tab 順序裡', () => {
    ball = make(200);
    const buttons = Array.from(ball.host.shadowRoot!.querySelectorAll('button'));
    expect(buttons.length).toBeGreaterThan(0);
    expect(buttons.every((b) => b.tabIndex === -1)).toBe(true);
  });
});
