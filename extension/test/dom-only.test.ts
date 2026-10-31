// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  insertTranslation,
  removeAllTranslations,
  setDisplayMode,
  watchNewBlocks,
  ORIG_CLASS,
  collectBlocks,
  getBlockText,
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

  it('F3 網站複製的已翻區塊,切到 only 時也會替換', () => {
    document.body.innerHTML = '<p id="src">Getting started guide here</p><div id="copy"></div>';
    const p = q('#src');
    translate(p, '入門指南');
    q('#copy').innerHTML = p.outerHTML.replace('id="src"', 'id="dup"');
    setDisplayMode('only', document);
    const dup = q('#dup');
    expect(dup.querySelector(origSel)?.textContent).toContain('Getting started guide here');
    expect(dup.querySelector(targetSel)?.parentElement).toBe(dup);
  });

  it('F4 元件以 slot 掛載的子元素在僅譯文時仍留在元件下,不被包走', () => {
    // 實測 Reddit:<shreddit-post> 用 slot 掛整卡連結與標題,被包進 atx-orig 後整卡點不進文章
    document.body.innerHTML = '<div id="card"><x-post><span slot="credit-bar">u/notfromholl posted</span><a slot="full-post-link" href="/p/1"></a><a slot="title" href="/p/1">Please help me optimize my character</a><div slot="text-body"><p>Body paragraph of the post.</p></div></x-post></div>';
    const host = q('x-post');
    host.attachShadow({ mode: 'open' }).innerHTML = '<slot name="credit-bar"></slot><slot name="full-post-link"></slot><slot name="title"></slot><slot name="text-body"></slot>';
    const card = q('#card');
    // slot 掛的標題自成一段(F5),實際送翻的是標題
    translate(q('a[slot=title]'), '請幫我優化我的角色');
    setDisplayMode('only', document);

    for (const c of Array.from(host.querySelectorAll("[slot]"))) expect(c.parentElement).toBe(host);
    expect(host.querySelector(`:scope > ${origSel}`)).toBeNull();
    expect(q('a[slot=title]').querySelector(origSel)?.textContent).toBe('Please help me optimize my character');
    expect(card.querySelector(targetSel)?.textContent).toBe('請幫我優化我的角色');
  });

  it('F5 卡片元件以 slot 掛的標題自成一段,僅譯文時譯文留在標題連結裡', () => {
    // 實測 Reddit 列表:整張卡片當一段時,譯文跑到按鈕列下方
    document.body.innerHTML = '<article id="card"><x-post><span slot="credit-bar">Posted by wertgzhj five days ago</span><a slot="title" href="/p/1">Made a leveling overlay for testers</a></x-post></article>';
    q('x-post').attachShadow({ mode: 'open' }).innerHTML = '<slot name="credit-bar"></slot><slot name="title"></slot>';
    const blocks = collectBlocks(document.body, { targetLang: 'zh-TW', minChars: 2, scope: 'page' });
    const title = q('a[slot=title]');
    expect(blocks).toContain(title);
    expect(blocks).not.toContain(q('#card'));
    translate(title, '做了一個升級提示工具,想找測試者');
    setDisplayMode('only', document);
    expect(title.parentElement).toBe(q('x-post'));
    expect(title.querySelector(targetSel)?.textContent).toBe('做了一個升級提示工具,想找測試者');
  });

  it('F6 沒有文字的元素(元件自己畫的返回鍵、圖示)在僅譯文時保持可見', () => {
    // 實測 Reddit 文章頁:返回鍵是空的元件,被包進 atx-orig 後消失
    document.body.innerHTML = '<div id="hdr"><x-back-button></x-back-button><span>Posted in pathofexiledev fifteen days ago</span></div>';
    const hdr = q('#hdr');
    translate(hdr, '十五天前發布於 pathofexiledev');
    setDisplayMode('only', document);
    expect(q('x-back-button').closest(origSel)).toBeNull();
    expect(q('span').closest(origSel)).not.toBeNull();
  });

  it('F7 螢幕閱讀器專用的隱藏文字不送翻', () => {
    // 實測 Reddit:<faceplate-screen-reader-content> 的「Go to ...」被翻出來顯示成「前往 pathofexiledev」
    document.body.innerHTML = '<div id="hdr"><a href="/r/x">r/pathofexiledev</a> fifteen days ago<faceplate-screen-reader-content>Go to pathofexiledev</faceplate-screen-reader-content><span class="sr-only">Hidden helper text</span></div>';
    expect(getBlockText(q('#hdr'))).toBe('r/pathofexiledev fifteen days ago');
  });
});
