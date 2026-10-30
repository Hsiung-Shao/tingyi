// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { collectBlocks, getBlockText, insertTranslation, removeAllTranslations, watchNewBlocks } from '../src/content/dom.ts';

const opts = { targetLang: 'zh-TW', minChars: 2 };
const texts = (b: HTMLElement[]) => b.map(getBlockText);
const tick = () => new Promise<void>((r) => setTimeout(r, 0));

afterEach(() => {
  document.body.innerHTML = '';
});

describe('巢狀區塊', () => {
  it('父項夾子清單時父項自己的文字也被收集', () => {
    document.body.innerHTML = '<ul><li>Parent item text<ul><li>Child item text</li></ul></li></ul>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['Parent item text', 'Child item text']);
  });

  it('父項的譯文插在子清單之前且還原後原樣', () => {
    document.body.innerHTML = '<ul><li id="p">Parent item text<ul><li>Child item text</li></ul></li></ul>';
    const before = document.body.innerHTML;
    const parent = document.getElementById('p')!;
    insertTranslation(parent, '父項');
    expect(parent.querySelector(':scope > font.atx-target')?.nextElementSibling?.tagName).toBe('UL');
    removeAllTranslations(document.body);
    expect(document.body.innerHTML).toBe(before);
  });

  it('已收集的容器內新增段落仍會被找到', async () => {
    document.body.innerHTML = '<div id="c">Intro text here<p>Para one text</p></div>';
    collectBlocks(document.body, opts).forEach((b) => (b.dataset.atxState = 'seen'));
    const got: HTMLElement[] = [];
    const stop = watchNewBlocks(document.body, opts, (b) => got.push(...b));
    const p = document.createElement('p');
    p.textContent = 'Added later paragraph';
    document.getElementById('c')!.appendChild(p);
    await tick();
    stop();
    expect(got).toContain(p);
  });
});

describe('已翻譯區塊的內容變更', () => {
  it('文字被改掉時移除舊譯文並重新排入', async () => {
    document.body.innerHTML = '<p id="p">Loading content</p>';
    const p = document.getElementById('p')!;
    p.dataset.atxState = 'done';
    insertTranslation(p, '載入中');
    const got: HTMLElement[] = [];
    const stop = watchNewBlocks(document.body, opts, (b) => got.push(...b));
    p.firstChild!.nodeValue = 'Real article text arrived';
    await tick();
    stop();
    expect(got).toContain(p);
    expect(p.querySelector('font.atx-target')).toBeNull();
  });

  it('插入譯文本身不會觸發重翻', async () => {
    document.body.innerHTML = '<p id="p">Stable paragraph text</p>';
    const p = document.getElementById('p')!;
    const got: HTMLElement[] = [];
    const stop = watchNewBlocks(document.body, opts, (b) => got.push(...b));
    p.dataset.atxState = 'done';
    insertTranslation(p, '穩定段落');
    await tick();
    stop();
    expect(got).toEqual([]);
  });
});

describe('目標語言判斷', () => {
  it('lang=ja 頁面的全漢字標題仍被收集', () => {
    document.body.innerHTML = '<h1 lang="ja">東京都知事選挙結果</h1>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['東京都知事選挙結果']);
  });

  it('目標繁中時簡體段落仍被收集', () => {
    document.body.innerHTML = '<p>这是一个简体中文的句子</p><p>這是繁體中文的句子</p>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['这是一个简体中文的句子']);
  });
});
