// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  collectBlocks,
  getBlockText,
  insertTranslation,
  setDisplayMode,
  INLINE_CLASS,
  TARGET_CLASS,
} from '../src/content/dom.ts';
import { isCodeLike } from '../../packages/core/src/lang.ts';

const base = { targetLang: 'zh-TW', minChars: 2 };
const texts = (blocks: HTMLElement[]) => blocks.map((b) => getBlockText(b));
const targetOf = (el: Element) => el.querySelector<HTMLElement>(`font.${TARGET_CLASS}`);

beforeEach(() => {
  document.body.innerHTML = '';
  document.documentElement.removeAttribute('data-atx-mode');
});
afterEach(() => {
  document.body.innerHTML = '';
  document.documentElement.removeAttribute('data-atx-mode');
});

const NAV_PAGE = `
  <nav><p>Nav paragraph text</p></nav>
  <header><p>Site header paragraph</p></header>
  <footer><p>Site footer paragraph</p></footer>
  <div role="navigation"><p>Role navigation paragraph</p></div>
  <div role="menu"><p>Role menu paragraph</p></div>
  <main><p>Main content paragraph</p></main>`;

describe('scope 範圍', () => {
  it('U1 主要內容模式略過 nav、header、footer、role 導覽區', () => {
    document.body.innerHTML = NAV_PAGE;
    const blocks = collectBlocks(document.body, { ...base, scope: 'main' });
    expect(texts(blocks)).toEqual(['Main content paragraph']);
  });

  it('U1 未指定 scope 時預設為主要內容模式', () => {
    document.body.innerHTML = NAV_PAGE;
    expect(texts(collectBlocks(document.body, base))).toEqual(['Main content paragraph']);
  });

  it('U2 article 內的 header 標題仍會收集', () => {
    document.body.innerHTML =
      '<header><p>Site header paragraph</p></header>' +
      '<article><header><h1>Article title here</h1></header><p>Article body text</p></article>';
    const blocks = collectBlocks(document.body, { ...base, scope: 'main' });
    expect(texts(blocks)).toEqual(['Article title here', 'Article body text']);
  });

  it('U2 main 內的 header 標題仍會收集', () => {
    document.body.innerHTML =
      '<footer><p>Site footer paragraph</p></footer><main><header><h1>Main title here</h1></header></main>';
    expect(texts(collectBlocks(document.body, { ...base, scope: 'main' }))).toEqual(['Main title here']);
  });

  it('U3 整頁模式連導覽區的段落也收集', () => {
    document.body.innerHTML = NAV_PAGE;
    const got = texts(collectBlocks(document.body, { ...base, scope: 'page' }));
    expect(got).toHaveLength(6);
    expect(got).toContain('Nav paragraph text');
    expect(got).toContain('Site footer paragraph');
    expect(got).toContain('Role menu paragraph');
    expect(got).toContain('Main content paragraph');
  });
});

describe('程式識別字', () => {
  it('U4 isCodeLike 對識別字回 true、對一般文字回 false', () => {
    for (const s of ['DeferredRequestInit', 'fetch()', 'snake_case', 'a.b.c']) {
      expect(isCodeLike(s), s).toBe(true);
    }
    for (const s of ['Install', 'Hello world']) {
      expect(isCodeLike(s), s).toBe(false);
    }
  });

  it('U4 識別字段落不收集,一般文字照收', () => {
    document.body.innerHTML = `
      <p>DeferredRequestInit</p><p>fetch()</p><p>snake_case</p><p>a.b.c</p>
      <p>Install</p><p>Hello world</p>`;
    expect(texts(collectBlocks(document.body, { ...base, scope: 'page' }))).toEqual(['Install', 'Hello world']);
  });

  it('U5 全部文字在 code 裡的區塊不收集,code 後還有文字的收集', () => {
    document.body.innerHTML = '<ul><li><code>Headers</code></li><li><code>fetch()</code> method</li></ul>';
    const [, second] = Array.from(document.querySelectorAll('li'));
    const blocks = collectBlocks(document.body, { ...base, scope: 'page' });
    expect(blocks).toEqual([second]);
  });
});

describe('insertTranslation 樣式資訊', () => {
  it('U6 只有短標籤 li 的譯文帶 atx-inline', () => {
    document.body.innerHTML = `
      <ul><li>Blog</li></ul>
      <p>${'This is a long paragraph that keeps going. '.repeat(5)}</p>
      <h2>Overview</h2>`;
    const li = document.querySelector('li') as HTMLElement;
    const p = document.querySelector('p') as HTMLElement;
    const h2 = document.querySelector('h2') as HTMLElement;
    insertTranslation(li, '部落格');
    insertTranslation(p, '長段落譯文');
    insertTranslation(h2, '概觀');
    expect(INLINE_CLASS).toBe('atx-inline');
    expect(targetOf(li)?.classList.contains(INLINE_CLASS)).toBe(true);
    expect(targetOf(p)).not.toBeNull();
    expect(targetOf(p)?.classList.contains(INLINE_CLASS)).toBe(false);
    expect(targetOf(h2)).not.toBeNull();
    expect(targetOf(h2)?.classList.contains(INLINE_CLASS)).toBe(false);
  });

  it('U7 譯文的 --atx-fs 記住區塊原字級', () => {
    document.body.innerHTML = '<p style="font-size:18px">Styled paragraph text</p>';
    const p = document.querySelector('p') as HTMLElement;
    insertTranslation(p, '有字級的段落');
    expect(targetOf(p)?.style.getPropertyValue('--atx-fs')).toBe('18px');
  });

  it('U7 不同字級各自記住', () => {
    document.body.innerHTML = '<p style="font-size:24px">Bigger paragraph text</p>';
    const p = document.querySelector('p') as HTMLElement;
    insertTranslation(p, '大字級');
    expect(targetOf(p)?.style.getPropertyValue('--atx-fs')).toBe('24px');
  });
});

describe('setDisplayMode', () => {
  it('U8 only 時 html 有 data-atx-mode=only,切回 dual 移除', () => {
    setDisplayMode('only', document);
    expect(document.documentElement.getAttribute('data-atx-mode')).toBe('only');
    setDisplayMode('dual', document);
    expect(document.documentElement.hasAttribute('data-atx-mode')).toBe(false);
  });

  it('U8 dual 與 only 可反覆切換', () => {
    setDisplayMode('only', document);
    setDisplayMode('dual', document);
    setDisplayMode('only', document);
    expect(document.documentElement.getAttribute('data-atx-mode')).toBe('only');
  });
});
