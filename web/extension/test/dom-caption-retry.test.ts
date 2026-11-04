// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { markCaptionPlain, markTranslateError, setDisplayMode, watchNewBlocks } from '../src/content/dom.ts';

/** YouTube 字幕框結構,沿用 dom-caption.test.ts */
const CAPTION = `<div id="movie_player" style="font-size: 11px">
<div class="ytp-caption-window-container"><div class="caption-window ytp-caption-window-bottom" id="caption-window-_0" style="text-align: center; width: 640px;">
<span class="captions-text" style="display: block;">
<span class="caption-visual-line" style="display: block;"><span class="ytp-caption-segment" style="display: inline-block; white-space: pre-wrap; font-size: 33.48px; color: rgb(255, 255, 255);">coming because they released this big</span></span>
</span></div></div></div>`;
const opts = { targetLang: 'zh-TW', minChars: 2, scope: 'page' } as const;
const tick = () => new Promise<void>((r) => setTimeout(r, 0));

function mount() {
  document.body.innerHTML = CAPTION;
  return {
    win: document.querySelector<HTMLElement>('.caption-window')!,
    seg: document.querySelector<HTMLElement>('.ytp-caption-segment')!,
  };
}

afterEach(() => {
  setDisplayMode('dual', document);
  document.body.innerHTML = '';
});

describe('字幕框標記不需翻譯', () => {
  // C1
  it('中文字幕標成照常顯示原文,英文與空字幕不標', () => {
    const { win, seg } = mount();
    seg.textContent = '這一句本來就是中文字幕';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(true); // 沒標會讓原生字幕被藏起來又沒有譯文

    seg.textContent = 'coming because they released this big';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(false);

    seg.textContent = '這一句本來就是中文字幕';
    markCaptionPlain(document.body, opts);
    seg.textContent = '';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(false); // 空字幕不該標
  });

  // C2
  it('同一個框換句後依新文字更新標記', () => {
    const { win, seg } = mount();
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(false);

    seg.textContent = '這一句本來就是中文字幕';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(true);

    seg.textContent = 'Another English caption line';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(false); // 沒清掉會讓新的英文句不翻譯
  });

  it('已有翻譯狀態的框不標、並移除舊標記', () => {
    const { win, seg } = mount();
    seg.textContent = '這一句本來就是中文字幕';
    win.dataset.atxState = 'seen';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(false);

    win.removeAttribute('data-atx-state');
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(true);
    win.dataset.atxState = 'done';
    markCaptionPlain(document.body, opts);
    expect(win.hasAttribute('data-atx-plain')).toBe(false);
  });
});

describe('翻譯失敗的字幕框', () => {
  // C3
  it('換句後清掉錯誤狀態並重新送翻', async () => {
    const { win, seg } = mount();
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, { ...opts, captions: 'only' }, (b) => received.push(b));
    win.dataset.atxState = 'error';
    await tick();
    received.length = 0;

    seg.textContent = 'A brand new English caption sentence';
    await tick();
    stop();

    expect(received.flat()).toContain(win); // 失敗後不重送,這個字幕框整段影片都不會再翻
    expect(win.dataset.atxState).not.toBe('error');
  });

  it('文字沒變時錯誤狀態維持不變', async () => {
    const { win } = mount();
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, { ...opts, captions: 'only' }, (b) => received.push(b));
    win.dataset.atxState = 'error';
    await tick();
    await tick();
    stop();
    expect(received.flat()).not.toContain(win); // 只改屬性就重送會讓失敗的句子無限重試
    expect(win.dataset.atxState).toBe('error');
  });

  // C4
  it('字幕框先清空再填字仍會送翻', async () => {
    const { win, seg } = mount();
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, { ...opts, captions: 'only' }, (b) => received.push(b));

    seg.textContent = '';
    await tick();
    expect(received.flat()).not.toContain(win); // 空字幕沒東西可翻

    seg.textContent = 'A brand new English caption sentence';
    await tick();
    stop();
    expect(received.flat()).toContain(win); // 清空後填字沒送翻,這段字幕會永遠沒譯文
  });

  // C5
  it('翻譯失敗的字幕框文字沒變就不重送,換句才重送', async () => {
    const { win, seg } = mount();
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, { ...opts, captions: 'only' }, (b) => received.push(b));
    markTranslateError(win, seg.textContent ?? '');
    await tick();
    received.length = 0;

    // 頁面把同樣內容的行換成新元素:文字沒變
    const line = win.querySelector('.caption-visual-line')!;
    line.replaceWith(line.cloneNode(true));
    await tick();
    expect(received.flat()).not.toContain(win); // 文字沒變就重送會讓失敗的句子無限重試
    expect(win.dataset.atxState).toBe('error');

    win.querySelector('.ytp-caption-segment')!.textContent = 'A different English caption sentence';
    await tick();
    stop();
    expect(received.flat()).toContain(win);
    expect(win.dataset.atxState).not.toBe('error');
  });
});
