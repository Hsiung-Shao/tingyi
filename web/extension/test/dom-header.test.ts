// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { collectBlocks, getBlockText } from '../src/content/dom.ts';

const opts = { targetLang: 'zh-TW', minChars: 2, scope: 'main' as const };
afterEach(() => {
  document.body.innerHTML = '';
});

describe('章節自己的 header/footer', () => {
  it('section 與 div 裡的標題列在主要內容模式仍會翻', () => {
    document.body.innerHTML = `
      <header><p>Site wide banner text</p></header>
      <section><header><h2>Section title here</h2></header><p>Section body text</p></section>
      <div class="post"><section><header><h1>Post title here</h1></header></section></div>
      <aside><footer><p>Aside footer note</p></footer></aside>
      <footer><p>Site wide footer text</p></footer>`;
    expect(collectBlocks(document.body, opts).map(getBlockText)).toEqual([
      'Section title here',
      'Section body text',
      'Post title here',
      'Aside footer note',
    ]);
  });
});
