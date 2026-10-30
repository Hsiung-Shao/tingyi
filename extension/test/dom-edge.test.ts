// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { collectBlocks, getBlockText, insertTranslation, removeAllTranslations, resetForRetranslate, watchNewBlocks } from '../src/content/dom.ts';

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

  it('同一區塊文字反覆變動時最多重翻 3 次', async () => {
    document.body.innerHTML = '<p id="p">Updated 0 seconds ago</p>';
    const p = document.getElementById('p')!;
    p.dataset.atxState = 'done';
    insertTranslation(p, '0 秒前更新');
    let reported = 0;
    const stop = watchNewBlocks(document.body, opts, (b) => {
      reported += b.length;
      // 模擬 content 端重翻完成
      p.dataset.atxState = 'done';
      insertTranslation(p, '譯文');
    });
    for (let i = 1; i <= 6; i++) {
      p.firstChild!.nodeValue = `Updated ${i} seconds ago`;
      await tick();
    }
    stop();
    expect(reported).toBe(3);
  });

  it('達到重翻上限時移除舊譯文,不留下對不上原文的譯文', () => {
    document.body.innerHTML = '<p id="p">Page one title</p>';
    const p = document.getElementById('p')!;
    for (let i = 0; i < 3; i++) {
      insertTranslation(p, `譯${i}`);
      expect(resetForRetranslate(p)).toBe(true);
    }
    insertTranslation(p, '第三次的譯文');
    p.dataset.atxState = 'done';
    expect(resetForRetranslate(p)).toBe(false);
    expect(p.querySelector('font.atx-target')).toBeNull();
    expect(p.dataset.atxState).toBe('stale');
  });

  it('安靜超過 30 秒後重翻次數重新計算', () => {
    vi.useFakeTimers();
    try {
      document.body.innerHTML = '<p id="p">Reused list item</p>';
      const p = document.getElementById('p')!;
      for (let i = 0; i < 3; i++) expect(resetForRetranslate(p)).toBe(true);
      expect(resetForRetranslate(p)).toBe(false);
      vi.advanceTimersByTime(31_000);
      expect(resetForRetranslate(p)).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('送出後到插入前文字被改時回報譯文已過期', () => {
    document.body.innerHTML = '<p id="p">Text changed while translating</p>';
    const p = document.getElementById('p')!;
    expect(insertTranslation(p, '舊譯文', 'Original text that was sent')).toBe(true);
    expect(insertTranslation(p, '新譯文', 'Text changed while translating')).toBe(false);
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

describe('行內程式碼', () => {
  it('段落內的行內 code 與 kbd 文字保留在送翻文字中', () => {
    document.body.innerHTML = '<p>Call <code>initialize()</code> before you press <kbd>Ctrl</kbd>+<kbd>S</kbd> to save.</p>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['Call initialize() before you press Ctrl+S to save.']);
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
