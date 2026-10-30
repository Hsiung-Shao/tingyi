// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  insertTranslation,
  removeAllTranslations,
  setDisplayMode,
  watchNewBlocks,
  ORIG_CLASS,
  TARGET_CLASS,
} from '../src/content/dom.ts';

const origSel = `font.${ORIG_CLASS}`;
const targetSel = `font.${TARGET_CLASS}`;

const clear = () => {
  setDisplayMode('dual', document);
  document.body.innerHTML = '';
  document.documentElement.removeAttribute('data-atx-mode');
};
beforeEach(clear);
afterEach(clear);

/** 插入譯文並標記完成(照 content script 的做法)。 */
function translate(el: HTMLElement, text: string, source?: string): void {
  insertTranslation(el, text, source);
  el.dataset.atxState = 'done';
}

const q = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T>(sel) as T;
const nonTarget = (el: Element) =>
  Array.from(el.childNodes).filter((n) => !(n instanceof HTMLElement && n.classList.contains(TARGET_CLASS)));

describe('僅譯文模式的原文包裝', () => {
  it('R1 切到 only 時原文節點進 atx-orig,譯文緊鄰容器', () => {
    document.body.innerHTML = '<p>Hello <b>world</b></p>';
    const p = q('p');
    const text = p.firstChild as Text;
    const b = q('b');
    translate(p, '你好世界');
    setDisplayMode('only', document);

    const orig = p.querySelector(origSel);
    expect(orig).not.toBeNull();
    expect(orig!.contains(text)).toBe(true);
    expect(orig!.contains(b)).toBe(true);
    const target = p.querySelector<HTMLElement>(targetSel)!;
    expect(target).not.toBeNull();
    expect(target.parentElement).toBe(p);
    expect(orig!.nextSibling === target || orig!.previousSibling === target).toBe(true);
  });

  it('R2 切回 dual 時原始子節點原樣還原且無 atx-orig', () => {
    document.body.innerHTML = '<p>Hello <b>world</b></p>';
    const p = q('p');
    const before = Array.from(p.childNodes);
    translate(p, '你好世界');
    setDisplayMode('only', document);
    expect(p.querySelector(origSel)).not.toBeNull();
    setDisplayMode('dual', document);

    expect(p.querySelector(origSel)).toBeNull();
    const after = nonTarget(p);
    expect(after.length).toBe(before.length);
    before.forEach((n, i) => expect(after[i]).toBe(n));
  });

  it('R3 巢狀 li 只包住自己的文字,子 ul 不被隱藏且在譯文之後', () => {
    document.body.innerHTML = '<ul><li id="par">Parent text<ul><li id="kid">Child item</li></ul></li></ul>';
    const par = q('#par');
    const kid = q('#kid');
    const innerUl = par.querySelector('ul')!;
    const parentText = par.firstChild as Text;
    translate(par, '父項目');
    translate(kid, '子項目');
    setDisplayMode('only', document);

    const orig = Array.from(par.children).find((c) => c.classList.contains(ORIG_CLASS));
    expect(orig).toBeDefined();
    expect(orig!.contains(parentText)).toBe(true);
    expect(innerUl.closest(origSel)).toBeNull();
    const parentTarget = Array.from(par.children).find((c) => c.classList.contains(TARGET_CLASS))!;
    expect(parentTarget).toBeDefined();
    expect(parentTarget.compareDocumentPosition(innerUl) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('R4 img 與 input 不進 atx-orig,只有文字被包住', () => {
    document.body.innerHTML = '<p><img src="x.png"> Search here <input></p>';
    const p = q('p');
    const textNode = Array.from(p.childNodes).find(
      (n) => n.nodeType === Node.TEXT_NODE && (n.nodeValue ?? '').includes('Search here'),
    )!;
    translate(p, '在這裡搜尋');
    setDisplayMode('only', document);

    expect(q('img').closest(origSel)).toBeNull();
    expect(q('input').closest(origSel)).toBeNull();
    const orig = p.querySelector(origSel);
    expect(orig).not.toBeNull();
    expect(orig!.contains(textNode)).toBe(true);
  });

  it('R5 li 內 a 包整行時,譯文放在原本那個 a 裡', () => {
    document.body.innerHTML = '<ul><li><a href="/blog">Blog posts</a></li></ul>';
    const li = q('li');
    const a = q('a');
    translate(li, '部落格文章');
    setDisplayMode('only', document);

    const target = li.querySelector<HTMLElement>(targetSel);
    expect(target).not.toBeNull();
    expect(target!.closest('a')).toBe(a);
  });

  it('R6 譯文不含連結,原文中的 a 一併被隱藏', () => {
    document.body.innerHTML = '<p>Click <a href="https://x.com">Test</a> now please</p>';
    const p = q('p');
    translate(p, '現在請點擊測試');
    setDisplayMode('only', document);

    expect(q('a').closest(origSel)).not.toBeNull();
    const target = p.querySelector<HTMLElement>(targetSel)!;
    expect(target).not.toBeNull();
    expect(target.querySelector('a')).toBeNull();
  });

  it('R7 譯文含原網址時,網址被做成單一新連結且不帶 id', () => {
    document.body.innerHTML =
      '<p>See <a id="lnk" href="https://ex.com/d">https://ex.com/d</a> for details</p>';
    const p = q('p');
    const zh = '詳情請見 https://ex.com/d';
    translate(p, zh);

    const target = p.querySelector<HTMLElement>(targetSel)!;
    expect(target).not.toBeNull();
    const links = target.querySelectorAll('a');
    expect(links.length).toBe(1);
    expect(links[0]!.textContent).toBe('https://ex.com/d');
    expect(links[0]!.getAttribute('href')).toBe('https://ex.com/d');
    expect(links[0]!.hasAttribute('id')).toBe(false);
    expect(target.textContent).toBe(zh);
  });

  it('R8 譯文不含網址時,原連結在 only 模式維持可見', () => {
    document.body.innerHTML =
      '<p>See <a id="lnk" href="https://ex.com/d">https://ex.com/d</a> for details</p>';
    const p = q('p');
    translate(p, '詳情請見網站');
    setDisplayMode('only', document);

    expect(q('#lnk').closest(origSel)).toBeNull();
  });

  it('R9 已在 only 模式時新插入譯文,原文立即被包住', () => {
    setDisplayMode('only', document);
    document.body.innerHTML = '<p>Hello there friend</p>';
    const p = q('p');
    const text = p.firstChild as Text;
    insertTranslation(p, '哈囉朋友');

    const orig = p.querySelector(origSel);
    expect(orig).not.toBeNull();
    expect(orig!.contains(text)).toBe(true);
  });

  it('R10 removeAllTranslations 清掉譯文與包裝並還原原始節點', () => {
    document.body.innerHTML = '<p>Hello <b>world</b></p>';
    const p = q('p');
    const before = Array.from(p.childNodes);
    translate(p, '你好世界');
    setDisplayMode('only', document);
    removeAllTranslations(document.body);

    expect(document.querySelector(origSel)).toBeNull();
    expect(document.querySelector(targetSel)).toBeNull();
    expect(p.childNodes.length).toBe(before.length);
    before.forEach((n, i) => expect(p.childNodes[i]).toBe(n));
    expect(p.hasAttribute('data-atx-state')).toBe(false);
  });

  it('R11 only 模式下被包住的文字被改寫,舊譯文與包裝清掉並重新交給 onBlocks', async () => {
    document.body.innerHTML = '<p>Loading the article</p>';
    const p = q('p');
    const text = p.firstChild as Text;
    translate(p, '載入文章中', 'Loading the article');
    setDisplayMode('only', document);

    const got: HTMLElement[] = [];
    const stop = watchNewBlocks(
      document.body,
      { targetLang: 'zh-TW', minChars: 2, scope: 'page' },
      (blocks) => got.push(...blocks),
    );
    try {
      text.nodeValue = 'Article body is here now';
      for (let i = 0; i < 50 && !got.includes(p); i++) await new Promise((r) => setTimeout(r, 10));

      expect(got).toContain(p);
      expect(p.querySelector(origSel)).toBeNull();
      expect(p.querySelector(targetSel)).toBeNull();
    } finally {
      stop();
    }
  });
});

describe('僅譯文模式的審查修正', () => {
  it('F1 第一段原文在含圖示的連結裡時,譯文不被塞進該連結', () => {
    document.body.innerHTML = '<p><a href="/u"><img src="a.png"> Alice</a> said the weather is nice today.</p>';
    const p = q('p');
    translate(p, '愛麗絲說今天天氣很好。');
    setDisplayMode('only', document);
    const target = p.querySelector<HTMLElement>(targetSel)!;
    expect(target.closest('a')).toBeNull();
    expect(target.parentElement).toBe(p);
  });

  it('F2 網站複製了已包裝的區塊,關閉翻譯後複本的原文恢復可見', () => {
    document.body.innerHTML = '<p id="src">Getting started guide here</p><div id="sticky"></div>';
    const p = q('#src');
    translate(p, '入門指南');
    setDisplayMode('only', document);
    q('#sticky').innerHTML = p.innerHTML;
    removeAllTranslations(document.body);
    expect(document.querySelector(origSel)).toBeNull();
    expect(q('#sticky').textContent).toContain('Getting started guide here');
  });
});
