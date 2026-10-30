// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  collectBlocks,
  getBlockText,
  insertTranslation,
  removeAllTranslations,
  watchNewBlocks,
  TARGET_CLASS,
} from '../src/content/dom.ts';

const opts = { targetLang: 'zh-TW', minChars: 2 };

beforeEach(() => {
  document.body.innerHTML = '';
});
afterEach(() => {
  document.body.innerHTML = '';
});

const texts = (blocks: HTMLElement[]) => blocks.map((b) => getBlockText(b));
const tick = () => new Promise<void>((r) => setTimeout(r, 0));

describe('collectBlocks', () => {
  it('T1 只擷取 p、h2、li 的文字並略過 script、pre、code', () => {
    document.body.innerHTML = `
      <h2>Heading two text</h2>
      <p>Paragraph one is here</p>
      <ul><li>List item number one</li></ul>
      <script>var scriptText = "should not be translated";</script>
      <pre>preformatted block of text</pre>
      <code>inline code sample here</code>`;
    const blocks = collectBlocks(document.body, opts);
    expect(texts(blocks)).toEqual(['Heading two text', 'Paragraph one is here', 'List item number one']);
    expect(blocks.map((b) => b.tagName)).toEqual(['H2', 'P', 'LI']);
  });

  it('T1 p 內的行內元素文字併入同一段且空白壓成單一空格', () => {
    document.body.innerHTML = '<p>Hello   <b>bold</b>\n  world</p>';
    const blocks = collectBlocks(document.body, opts);
    expect(blocks).toHaveLength(1);
    expect(getBlockText(blocks[0]!)).toBe('Hello bold world');
  });

  it('T1 pre 內的段落與 contenteditable 內容不收集', () => {
    document.body.innerHTML = `
      <pre><p>text inside pre block</p></pre>
      <div contenteditable="true"><p>editable paragraph text</p></div>
      <p>Normal paragraph text</p>`;
    expect(texts(collectBlocks(document.body, opts))).toEqual(['Normal paragraph text']);
  });

  it('T1 只含行內元素與文字的 div 視為區塊', () => {
    document.body.innerHTML = '<div>Plain div with <em>inline</em> text</div>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['Plain div with inline text']);
  });

  it('T2 太短的段落不收集', () => {
    document.body.innerHTML = '<p>A</p><p>Long enough sentence</p>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['Long enough sentence']);
  });

  it('T2 已是繁中的段落不收集', () => {
    document.body.innerHTML = '<p>這是一段已經是繁體中文的句子。</p><p>This one needs translating</p>';
    expect(texts(collectBlocks(document.body, opts))).toEqual(['This one needs translating']);
  });

  it('T2 混合多種段落時只留下該翻的', () => {
    document.body.innerHTML = `
      <p>A</p>
      <p>中文段落不需要再翻譯了</p>
      <p>Keep this English paragraph</p>
      <h3>Another English heading</h3>`;
    expect(texts(collectBlocks(document.body, opts))).toEqual([
      'Keep this English paragraph',
      'Another English heading',
    ]);
  });

  it('T3 已有譯文的區塊不會再被收集', () => {
    document.body.innerHTML = '<p>Translate me please</p>';
    const first = collectBlocks(document.body, opts);
    expect(first).toHaveLength(1);
    insertTranslation(first[0]!, '請翻譯我');
    expect(collectBlocks(document.body, opts)).toEqual([]);
  });
});

describe('insertTranslation / getBlockText', () => {
  it('T3 譯文插在區塊最後面且不影響原文文字', () => {
    document.body.innerHTML = '<p>Original text <b>here</b></p>';
    const p = document.querySelector('p') as HTMLElement;
    insertTranslation(p, '原文在這裡');
    const target = p.querySelector(`.${TARGET_CLASS}`);
    expect(target).not.toBeNull();
    expect(target?.textContent).toBe('原文在這裡');
    expect(target?.tagName).toBe('FONT');
    expect(p.lastElementChild).toBe(target);
    expect(getBlockText(p)).toBe('Original text here');
  });

  it('T3 同一段插入兩次只有一個 atx-target 且文字被更新', () => {
    document.body.innerHTML = '<p>Original text here</p>';
    const p = document.querySelector('p') as HTMLElement;
    insertTranslation(p, '第一次');
    insertTranslation(p, '第二次');
    const targets = p.querySelectorAll('.atx-target');
    expect(targets).toHaveLength(1);
    expect(targets[0]?.textContent).toBe('第二次');
  });
});

describe('removeAllTranslations', () => {
  it('T4 關閉翻譯後頁面沒有 atx-target 且原文不變', () => {
    document.body.innerHTML = '<p>First paragraph text</p><ul><li>Second item text</li></ul>';
    const before = document.body.innerHTML;
    const blocks = collectBlocks(document.body, opts);
    expect(blocks).toHaveLength(2);
    blocks.forEach((b) => insertTranslation(b, '譯文'));
    expect(document.querySelectorAll('.atx-target')).toHaveLength(2);
    removeAllTranslations(document.body);
    expect(document.querySelectorAll('.atx-target')).toHaveLength(0);
    expect(document.querySelector('[data-atx-state]')).toBeNull();
    expect(document.body.innerHTML).toBe(before);
  });

  it('T4 還原後區塊可以被重新收集', () => {
    document.body.innerHTML = '<p>Collect me again later</p>';
    const [p] = collectBlocks(document.body, opts);
    expect(p).toBeDefined();
    insertTranslation(p!, '再收集');
    removeAllTranslations(document.body);
    expect(texts(collectBlocks(document.body, opts))).toEqual(['Collect me again later']);
  });
});

describe('watchNewBlocks', () => {
  it('T5 動態 append 的段落進入翻譯佇列', async () => {
    document.body.innerHTML = '<p>Existing paragraph text</p>';
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, opts, (b) => received.push(b));
    const p = document.createElement('p');
    p.textContent = 'Freshly added paragraph';
    document.body.appendChild(p);
    await tick();
    stop();
    const all = received.flat();
    expect(all).toContain(p);
    expect(texts(all)).not.toContain('Existing paragraph text');
  });

  it('T5 新增容器內的巢狀段落也被找到', async () => {
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, opts, (b) => received.push(b));
    const wrap = document.createElement('section');
    wrap.innerHTML = '<p>Nested paragraph one</p><p>Nested paragraph two</p>';
    document.body.appendChild(wrap);
    await tick();
    stop();
    expect(texts(received.flat())).toEqual(['Nested paragraph one', 'Nested paragraph two']);
  });

  it('T5 停止監看後新增的段落不再通知', async () => {
    const received: HTMLElement[][] = [];
    const stop = watchNewBlocks(document.body, opts, (b) => received.push(b));
    const first = document.createElement('p');
    first.textContent = 'Added before stop call';
    document.body.appendChild(first);
    await tick();
    expect(texts(received.flat())).toEqual(['Added before stop call']);
    stop();
    const second = document.createElement('p');
    second.textContent = 'Added after stop call';
    document.body.appendChild(second);
    await tick();
    await tick();
    expect(texts(received.flat())).toEqual(['Added before stop call']);
  });
});
