// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { collectBlocks, insertTranslation, setDisplayMode, TARGET_CLASS } from '../src/content/dom.ts';

/** YouTube 字幕的實際結構(2026-10 量到):字級、顏色、底色寫在每段字幕的 span 上,字幕框本身是播放器的小字。
 * jsdom 模擬不了 text-shadow(寫入即失效),外框改由實機驗 */
const CAPTION = `<div id="movie_player" style="font-size: 11px">
<div class="ytp-caption-window-container"><div class="caption-window ytp-caption-window-bottom" id="caption-window-_0" style="text-align: center; width: 640px;">
<span class="captions-text" style="display: block;">
<span class="caption-visual-line" style="display: block;"><span class="ytp-caption-segment" style="display: inline-block; white-space: pre-wrap; background: rgba(8, 8, 8, 0.75); font-size: 33.48px; color: rgb(255, 255, 255); font-family: Roboto, Arial, sans-serif;">coming because they released this big</span></span>
<span class="caption-visual-line" style="display: block;"><span class="ytp-caption-segment" style="display: inline-block; white-space: pre-wrap; background: rgba(8, 8, 8, 0.75); font-size: 33.48px; color: rgb(255, 255, 255); font-family: Roboto, Arial, sans-serif;">teaser a couple of months ago at</span></span>
</span></div></div></div>`;

afterEach(() => {
  setDisplayMode('dual', document);
  document.body.innerHTML = '';
});

describe('YouTube 字幕', () => {
  it('字幕譯文沿用字幕的字級、字型、顏色與底色', () => {
    document.body.innerHTML = CAPTION;
    const [block] = collectBlocks(document.body, { targetLang: 'zh-TW', minChars: 2, scope: 'page' });
    expect(block?.classList.contains('caption-window')).toBe(true);
    insertTranslation(block!, '因為他們幾個月前發布了這個大預告');
    const style = getComputedStyle(block!.querySelector<HTMLElement>(`font.${TARGET_CLASS}`)!);
    expect(style.fontSize).toBe('33.48px');
    expect(style.color).toBe('rgb(255, 255, 255)');
    expect(style.backgroundColor).toBe('rgba(8, 8, 8, 0.75)');
    expect(style.fontFamily).toBe('Roboto, Arial, sans-serif');
  });

  it('一般段落的譯文不加任何字幕樣式', () => {
    document.body.innerHTML = '<p style="font-size: 18px">Plain paragraph text here.</p>';
    const p = document.querySelector('p')!;
    insertTranslation(p, '一般段落');
    expect(p.querySelector<HTMLElement>(`font.${TARGET_CLASS}`)!.getAttribute('style')).toBeNull();
  });
});
