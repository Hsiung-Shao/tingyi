// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { collectBlocks, setDisplayMode } from '../src/content/dom.ts';

/** YouTube 字幕框結構,沿用 dom-caption.test.ts */
const CAPTION = `<div id="movie_player" style="font-size: 11px">
<div class="ytp-caption-window-container"><div class="caption-window ytp-caption-window-bottom" id="caption-window-_0" style="text-align: center; width: 640px;">
<span class="captions-text" style="display: block;">
<span class="caption-visual-line" style="display: block;"><span class="ytp-caption-segment" style="display: inline-block; white-space: pre-wrap; font-size: 33.48px; color: rgb(255, 255, 255);">coming because they released this big</span></span>
<span class="caption-visual-line" style="display: block;"><span class="ytp-caption-segment" style="display: inline-block; white-space: pre-wrap; font-size: 33.48px; color: rgb(255, 255, 255);">teaser a couple of months ago at</span></span>
</span></div></div></div>`;
const PAGE = `<p id="para">Plain paragraph text that is long enough.</p>${CAPTION}`;
const base = { targetLang: 'zh-TW', minChars: 2, scope: 'page' } as const;
const inCaption = (el: HTMLElement) => el.closest('.ytp-caption-window-container') !== null;

afterEach(() => {
  setDisplayMode('dual', document);
  document.body.innerHTML = '';
});

describe('YouTube 字幕篩選', () => {
  // U11
  it('exclude 只收一般段落、不收字幕框內元素', () => {
    document.body.innerHTML = PAGE;
    const blocks = collectBlocks(document.body, { ...base, captions: 'exclude' });
    expect(blocks.map((b) => b.id)).toContain('para'); // 全擋掉會漏翻頁面
    expect(blocks.filter(inCaption)).toEqual([]); // 沒擋會把字幕當頁面翻
  });

  // U12
  it('only 只收字幕框內元素、不收一般段落', () => {
    document.body.innerHTML = PAGE;
    const blocks = collectBlocks(document.body, { ...base, captions: 'only' });
    expect(blocks.length).toBeGreaterThan(0); // 全擋掉會讓字幕翻不出來
    expect(blocks.every(inCaption)).toBe(true);
    expect(blocks.map((b) => b.id)).not.toContain('para');
  });

  // U13
  it('未指定篩選時字幕與段落都收', () => {
    document.body.innerHTML = PAGE;
    const blocks = collectBlocks(document.body, base);
    expect(blocks.map((b) => b.id)).toContain('para');
    expect(blocks.some(inCaption)).toBe(true); // 預設行為被改掉會讓既有字幕翻譯失效
  });
});
