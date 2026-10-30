// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createBall, isGenuineTarget, type BallHandle } from '../src/content/ball.ts';

let ball: BallHandle | undefined;
afterEach(() => {
  ball?.destroy();
  ball = undefined;
  vi.restoreAllMocks();
});

const rect = (x: number, y: number, w: number, h: number) =>
  ({ x, y, left: x, top: y, width: w, height: h, right: x + w, bottom: y + h, toJSON() {} }) as DOMRect;

function setup() {
  Object.defineProperty(window, 'innerWidth', { value: 1280, configurable: true });
  Object.defineProperty(window, 'innerHeight', { value: 800, configurable: true });
  ball = createBall(document, {
    onToggle() {},
    onMode() {},
    onScope() {},
    onOpenOptions() {},
    initial: { state: { enabled: false, done: 0, pending: 0, errors: 0 }, prefs: { displayMode: 'dual', scope: 'main' } },
  });
  const toggle = ball.host.shadowRoot!.querySelector<HTMLElement>('[data-atx="toggle"]')!;
  return { toggle };
}

describe('isGenuineTarget 防點擊劫持', () => {
  it('貼在右緣、正常大小、不透明的主按鈕算正常', () => {
    const { toggle } = setup();
    vi.spyOn(toggle, 'getBoundingClientRect').mockReturnValue(rect(1240, 360, 40, 40));
    expect(isGenuineTarget(toggle, ball!.host, window)).toBe(true);
  });

  it('被放大成全畫面的主按鈕不算', () => {
    const { toggle } = setup();
    vi.spyOn(toggle, 'getBoundingClientRect').mockReturnValue(rect(0, 0, 1280, 800));
    expect(isGenuineTarget(toggle, ball!.host, window)).toBe(false);
  });

  it('被移到畫面中央的主按鈕不算', () => {
    const { toggle } = setup();
    vi.spyOn(toggle, 'getBoundingClientRect').mockReturnValue(rect(600, 360, 40, 40));
    expect(isGenuineTarget(toggle, ball!.host, window)).toBe(false);
  });

  it('自己或 host 被設成透明時不算', () => {
    const { toggle } = setup();
    vi.spyOn(toggle, 'getBoundingClientRect').mockReturnValue(rect(1240, 360, 40, 40));
    toggle.style.opacity = '0';
    expect(isGenuineTarget(toggle, ball!.host, window)).toBe(false);
    toggle.style.opacity = '';
    ball!.host.style.setProperty('opacity', '0.01');
    expect(isGenuineTarget(toggle, ball!.host, window)).toBe(false);
  });

  it('host 被 transform 縮放時不算', () => {
    const { toggle } = setup();
    vi.spyOn(toggle, 'getBoundingClientRect').mockReturnValue(rect(1240, 360, 40, 40));
    ball!.host.style.setProperty('transform', 'scale(40)');
    expect(isGenuineTarget(toggle, ball!.host, window)).toBe(false);
  });
});
