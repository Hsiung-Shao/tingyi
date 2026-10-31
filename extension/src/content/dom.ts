import { isCodeLike, isLikelyTargetLang } from '@atx/core';
import type { DisplayMode, Scope } from '../shared/settings.ts';

export const TARGET_CLASS = 'atx-target';
/** 短標籤的譯文同行顯示時加的 class */
export const INLINE_CLASS = 'atx-inline';
/** 僅譯文模式下包住原文的隱藏容器 */
export const ORIG_CLASS = 'atx-orig';

export interface CollectOptions {
  targetLang: string;
  /** 段落去空白後最少字元數 */
  minChars: number;
  /**
   * main(預設):跳過 nav、[role=navigation|banner|menu|menubar|toolbar|contentinfo]、
   * 不在 article/main 裡的 header/footer;page:不跳過。
   */
  scope?: Scope;
}

/**
 * only:在 <html> 設 data-atx-mode="only",已翻區塊的原文包進隱藏容器、譯文佔原位;
 * dual:移除該屬性並解開包裝。
 */
export function setDisplayMode(mode: DisplayMode, doc: Document = document): void {
  if (mode === 'only') doc.documentElement.dataset.atxMode = 'only';
  else delete doc.documentElement.dataset.atxMode;
  for (const el of Array.from(doc.querySelectorAll<HTMLElement>('[data-atx-state="done"]'))) {
    if (mode === 'only') applyOnly(el);
    else restoreDual(el);
  }
}

/** 介面區塊(選單、工具列、頁首頁尾):主要內容模式整個子樹不翻 */
const CHROME_ROLES = new Set(['navigation', 'banner', 'menu', 'menubar', 'toolbar', 'contentinfo']);
function isChrome(el: Element): boolean {
  const tag = el.tagName.toUpperCase();
  if (tag === 'NAV') return true;
  if (CHROME_ROLES.has(el.getAttribute('role') ?? '')) return true;
  // 依 ARIA 對應:header/footer 只有不在 sectioning 內容裡時才是整站頁首頁尾;
  // 章節、文章自己的標題列(<section><header><h2>)要翻
  return (
    (tag === 'HEADER' || tag === 'FOOTER') &&
    !el.parentElement?.closest('article, aside, main, nav, section, [role=article], [role=main]')
  );
}

/** el 或它的祖先是否落在介面區塊(用在監看新增節點時) */
function inChrome(el: Element, opts: CollectOptions): boolean {
  if (opts.scope === 'page') return false;
  for (let p: Element | null = el; p; p = p.parentElement) if (isChrome(p)) return true;
  return false;
}

/** 短標籤的門檻:不超過這個長度、而且不是標題的區塊,譯文同行顯示 */
const INLINE_MAX_CHARS = 24;
const CODE_TAGS = new Set(['CODE', 'KBD', 'SAMP', 'TT', 'VAR']);

/** 本身就是一段文字的區塊 */
const LEAF_TAGS = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'LI', 'TD', 'TH', 'BLOCKQUOTE', 'DD', 'DT', 'FIGCAPTION', 'SUMMARY', 'CAPTION', 'LEGEND']);
/** 容器:裡面沒有其他區塊時才當成一段 */
const CONTAINER_TAGS = new Set(['DIV', 'SECTION', 'ARTICLE', 'MAIN', 'ASIDE', 'HEADER', 'FOOTER', 'NAV', 'FIGURE', 'DETAILS', 'LABEL']);
/** 整個子樹都不翻 */
const SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'PRE', 'CODE', 'KBD', 'SAMP', 'TEXTAREA', 'INPUT', 'SELECT', 'OPTION', 'BUTTON', 'SVG', 'MATH', 'CANVAS', 'VIDEO', 'AUDIO', 'IFRAME', 'OBJECT', 'HEAD']);
/** 段落文字裡要拿掉的:看不到或屬於表單的內容。行內 code/kbd 是句子的一部分,要保留 */
const HIDDEN_TEXT_TAGS = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'TEXTAREA', 'SELECT', 'OPTION']);
/** 判斷「裡面還有沒有區塊」用:多了 pre、ul、table 這類不翻但會斷段的元素 */
const BLOCKISH_SELECTOR = [...LEAF_TAGS, ...CONTAINER_TAGS, 'PRE', 'UL', 'OL', 'DL', 'TABLE', 'TR', 'HR']
  .map((t) => t.toLowerCase())
  .join(',');

function isSkipped(el: Element, opts: CollectOptions): boolean {
  if (SKIP_TAGS.has(el.tagName.toUpperCase())) return true;
  if (opts.scope !== 'page' && isChrome(el)) return true;
  if (el.classList.contains(TARGET_CLASS) || el.classList.contains('notranslate')) return true;
  if (el.getAttribute('translate') === 'no') return true;
  const ce = el.getAttribute('contenteditable');
  return ce !== null && ce !== 'false';
}

/** 每個區塊的譯文元素:僅譯文模式會把它搬進連結裡,不能只找直接子節點 */
const targets = new WeakMap<Element, HTMLElement>();
/** 僅譯文模式下每個區塊建立的原文容器,還原時逐一解開 */
const wrappers = new WeakMap<Element, HTMLElement[]>();
/** 僅譯文模式仍保留可見的元素:圖示、媒體與表單控制項 */
const KEEP_VISIBLE_SELECTOR = 'img,svg,picture,video,canvas,iframe,input,button,select,textarea';
const URL_TEXT = /^https?:\/\/\S+$/;

function targetOf(el: Element): HTMLElement | null {
  const t = targets.get(el);
  if (t && el.contains(t)) return t;
  // 網站複製的已翻區塊(輪播複本、黏性標題)不在紀錄裡,從 DOM 找回雙語位置的譯文
  const found = el.querySelector<HTMLElement>(`:scope > font.${TARGET_CLASS}`);
  if (found) targets.set(el, found);
  return found;
}

/** node 是否屬於 el 自己的行內內容(不在子區塊或譯文裡) */
function isOwnInline(el: Element, node: Node): boolean {
  for (let p = node.parentElement; p && p !== el; p = p.parentElement) {
    if (p.classList.contains(TARGET_CLASS) || p.matches(BLOCKISH_SELECTOR)) return false;
  }
  return true;
}

/** 區塊裡文字本身就是網址的連結(這類連結不替換) */
function urlLinks(el: Element): HTMLAnchorElement[] {
  return Array.from(el.querySelectorAll<HTMLAnchorElement>('a[href]')).filter(
    (a) => isOwnInline(el, a) && URL_TEXT.test(a.textContent?.trim() ?? ''),
  );
}

/** 譯文寫入 target;譯文裡出現原文的網址連結時,換成該連結的複本(網址照原樣、可以點) */
function fillTarget(el: Element, target: HTMLElement, text: string): void {
  target.textContent = text;
  for (const a of urlLinks(el)) {
    const url = a.textContent!.trim();
    for (const node of Array.from(target.childNodes)) {
      const i = node.nodeType === Node.TEXT_NODE ? (node.nodeValue ?? '').indexOf(url) : -1;
      if (i < 0) continue;
      const mid = (node as Text).splitText(i);
      mid.splitText(url.length);
      const clone = a.cloneNode(true) as HTMLElement;
      clone.removeAttribute('id');
      clone.querySelectorAll('[id]').forEach((n) => n.removeAttribute('id'));
      mid.replaceWith(clone);
      break;
    }
  }
}

/** 影片字幕的一段(YouTube):字級、顏色、底色寫在這個 span 上,字幕框本身只有播放器的小字 */
const CAPTION_SEGMENT_SELECTOR = '.ytp-caption-segment';
const CAPTION_CLASS = 'atx-caption';

/** 字幕的譯文照抄字幕那段的字級、字型、顏色與底色 */
function matchCaptionStyle(el: Element, target: HTMLElement): void {
  const seg = el.querySelector(CAPTION_SEGMENT_SELECTOR);
  if (!seg) return;
  const s = getComputedStyle(seg);
  target.classList.add(CAPTION_CLASS);
  target.style.fontSize = s.fontSize;
  target.style.fontFamily = s.fontFamily;
  target.style.color = s.color;
  target.style.backgroundColor = s.backgroundColor;
}

/** 譯文放回雙語位置:有子區塊時放在第一個子區塊前,否則放最後 */
function placeDual(el: Element, target: HTMLElement): void {
  const firstBlock = Array.from(el.children).find((c) => c !== target && c.matches(BLOCKISH_SELECTOR));
  el.insertBefore(target, firstBlock ?? null);
}

/**
 * 僅譯文:把區塊自己的行內原文包進隱藏的 <font class="atx-orig">,譯文放在第一段原文的位置。
 * 子區塊、圖示與表單控制項、譯文裡找不到的網址連結不包(維持可見)。
 * 整段文字都在同一個連結裡時,包裝和譯文都放進那個連結,譯文仍可點。
 */
function applyOnly(el: HTMLElement): void {
  restoreDual(el);
  const target = targetOf(el);
  if (!target) return;
  const text = getBlockText(el);
  const translated = target.textContent ?? '';
  const keptLinks = new Set<Element>(urlLinks(el).filter((a) => !translated.includes(a.textContent!.trim())));
  const soleLink = Array.from(el.querySelectorAll('a')).find(
    (a) => isOwnInline(el, a) && !keptLinks.has(a) && !a.querySelector(BLOCKISH_SELECTOR) &&
      (a.textContent ?? '').replace(/\s+/g, ' ').trim() === text,
  );
  const isKept = (n: Element) => n.matches(`${BLOCKISH_SELECTOR},${KEEP_VISIBLE_SELECTOR}`) || keptLinks.has(n);
  const hasKept = (n: Element) =>
    n.querySelector(`${BLOCKISH_SELECTOR},${KEEP_VISIBLE_SELECTOR}`) !== null || [...keptLinks].some((a) => n.contains(a));
  const doc = el.ownerDocument;
  const made: HTMLElement[] = [];
  const wrap = (parent: Element) => {
    let run: Node[] = [];
    const flush = (before: Node | null) => {
      if (run.length === 0) return;
      const w = doc.createElement('font');
      w.className = ORIG_CLASS;
      parent.insertBefore(w, before);
      w.append(...run);
      made.push(w);
      run = [];
    };
    for (const n of Array.from(parent.childNodes)) {
      if (n === target) flush(n);
      else if (n.nodeType !== Node.ELEMENT_NODE) run.push(n);
      else if (isKept(n as Element)) flush(n);
      else if (hasKept(n as Element)) {
        flush(n);
        wrap(n as Element);
      } else run.push(n);
    }
    flush(null);
  };
  const host = soleLink ?? el;
  wrap(host);
  if (made.length === 0) return;
  wrappers.set(el, made);
  // 譯文放在 host 這一層:第一段原文若在頭像連結、徽章這類行內元素裡,不能把整段譯文塞進去
  let top: Node | undefined = made.find((w) => (w.textContent ?? '').trim() !== '') ?? made[0];
  while (top && top.parentNode !== host) top = top.parentNode ?? undefined;
  host.insertBefore(target, top?.nextSibling ?? null);
}

/** 解開僅譯文的包裝,原文節點回到原位,譯文回到雙語位置 */
function restoreDual(el: HTMLElement): void {
  const made = wrappers.get(el);
  if (!made) return;
  wrappers.delete(el);
  for (const w of made) w.replaceWith(...Array.from(w.childNodes));
  const target = targetOf(el);
  if (target) placeDual(el, target);
}

/** 譯文對應的原文(送出時的文字);文字被頁面改掉時用來判斷要不要重翻 */
const sourceText = new WeakMap<Element, string>();
/**
 * 每個區塊因文字變動重翻的紀錄。計時器、股價這類一直變的文字不能無限重送;
 * 但 SPA 換頁、虛擬清單會重複使用同一個節點,所以用時間窗:安靜一段時間後重新計數。
 */
const retranslated = new WeakMap<Element, { count: number; last: number }>();
const MAX_RETRANSLATE = 3;
const RETRANSLATE_WINDOW_MS = 30_000;

/**
 * 原文已變、要重翻:清掉舊譯文與狀態並回 true。
 * 時間窗內超過次數上限就回 false,但同樣移除舊譯文並標成 stale(對不上原文的譯文比沒有更糟)。
 */
export function resetForRetranslate(el: HTMLElement): boolean {
  const now = Date.now();
  const prev = retranslated.get(el);
  const count = prev && now - prev.last < RETRANSLATE_WINDOW_MS ? prev.count : 0;
  retranslated.set(el, { count: Math.min(count + 1, MAX_RETRANSLATE + 1), last: now });
  restoreDual(el);
  targetOf(el)?.remove();
  targets.delete(el);
  if (count >= MAX_RETRANSLATE) {
    el.dataset.atxState = 'stale';
    sourceText.set(el, getBlockText(el));
    return false;
  }
  delete el.dataset.atxState;
  sourceText.delete(el);
  return true;
}

function isCandidate(el: Element, opts: CollectOptions): el is HTMLElement {
  const tag = el.tagName.toUpperCase();
  if (!LEAF_TAGS.has(tag) && !CONTAINER_TAGS.has(tag)) return false;
  if ((el as HTMLElement).dataset?.atxState !== undefined) return false;
  if (targetOf(el)) return false;
  const text = getBlockText(el);
  if (text.length < opts.minChars) return false;
  // API 名稱、程式識別字翻了也是原字,不送
  if (isCodeLike(text) || !hasTextOutsideCode(el)) return false;
  // 全漢字的日文標題看起來像中文,有 lang 標示就以它為準
  if (el.closest('[lang]')?.getAttribute('lang')?.toLowerCase().startsWith('ja')) return true;
  return !isLikelyTargetLang(text, opts.targetLang);
}

/** 區塊有沒有不在 code/kbd/samp/tt/var 裡的可見文字(全部是程式碼的區塊不送翻) */
function hasTextOutsideCode(el: Element): boolean {
  const walker = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (!n.nodeValue?.trim()) continue;
    let inCode = false;
    for (let p = n.parentElement; p && p !== el; p = p.parentElement) {
      const tag = p.tagName.toUpperCase();
      if (CODE_TAGS.has(tag) || p.classList.contains(TARGET_CLASS) || HIDDEN_TEXT_TAGS.has(tag)) {
        inCode = true;
        break;
      }
    }
    if (!inCode) return true;
  }
  return false;
}

function walk(el: Element, opts: CollectOptions, out: HTMLElement[], includeSelf: boolean): void {
  if (isSkipped(el, opts)) return;
  if (includeSelf) {
    const hasBlocks = el.querySelector(BLOCKISH_SELECTOR) !== null;
    // 有子區塊時,自己行內那段文字(例如 <li>父項<ul>…</ul></li>)也算一段
    if (isCandidate(el, opts)) out.push(el);
    if (!hasBlocks) return;
  }
  for (const child of Array.from(el.children)) walk(child, opts, out, true);
}

/**
 * 找出 root 底下該翻譯的「葉層區塊」:p、h1–h6、li、td、th、blockquote、dd、dt、figcaption、summary、
 * 以及只含行內元素與文字的 div。略過 SCRIPT/STYLE/PRE/CODE/TEXTAREA/SVG/MATH/NOSCRIPT 內的內容、
 * contenteditable、已有譯文或已收集過(data-atx-state)的區塊、太短或已是目標語言的段落。
 */
export function collectBlocks(root: Element, opts: CollectOptions): HTMLElement[] {
  const out: HTMLElement[] = [];
  if (root.closest(`[contenteditable]:not([contenteditable="false"]), .${TARGET_CLASS}`)) return out;
  walk(root, opts, out, false);
  return out;
}

/** 區塊要送翻的文字(含行內子元素文字,不含已插入的譯文與子區塊),空白壓成單一空格 */
export function getBlockText(el: Element): string {
  const doc = el.ownerDocument;
  const walker = doc.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      for (let p = node.parentElement; p && p !== el; p = p.parentElement) {
        if (p.classList.contains(TARGET_CLASS) || HIDDEN_TEXT_TAGS.has(p.tagName.toUpperCase())) return NodeFilter.FILTER_REJECT;
        if (p.matches(BLOCKISH_SELECTOR)) return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  let text = '';
  for (let n = walker.nextNode(); n; n = walker.nextNode()) text += n.nodeValue ?? '';
  return text.replace(/\s+/g, ' ').trim();
}

/**
 * 在區塊內插入 <font class="atx-target">:一般放最後面,有子區塊時放在第一個子區塊前;已存在就只更新文字。
 * 僅譯文模式下插入後直接替換原文位置。
 * source 是送翻時的原文;插入時原文已經不同(翻譯途中被頁面改掉)就回 true,表示譯文已過期。
 */
export function insertTranslation(el: HTMLElement, text: string, source?: string): boolean {
  let target = targetOf(el);
  const current = getBlockText(el);
  if (!target) {
    target = el.ownerDocument.createElement('font');
    target.className = TARGET_CLASS;
    // 選單項目、按鈕這類短標籤:譯文接在同一行,不另起一行撐開版面
    if (!/^H[1-6]$/.test(el.tagName.toUpperCase()) && current.length <= INLINE_MAX_CHARS) target.classList.add(INLINE_CLASS);
    matchCaptionStyle(el, target);
    placeDual(el, target);
    targets.set(el, target);
  }
  fillTarget(el, target, text);
  sourceText.set(el, source ?? current);
  if (el.ownerDocument.documentElement.dataset.atxMode === 'only') applyOnly(el);
  return source !== undefined && source !== current;
}

/** 移除 root 底下所有譯文、僅譯文包裝與 data-atx-state 標記,還原頁面 */
export function removeAllTranslations(root: Element): void {
  const marked = [root, ...Array.from(root.querySelectorAll<HTMLElement>('[data-atx-state]'))];
  marked.forEach((el) => restoreDual(el as HTMLElement));
  // 網站複製過的區塊(黏性標題、輪播複本)帶著我們的容器,但不在紀錄裡,也要解開
  root.querySelectorAll(`font.${ORIG_CLASS}`).forEach((w) => w.replaceWith(...Array.from(w.childNodes)));
  root.querySelectorAll(`font.${TARGET_CLASS}`).forEach((f) => f.remove());
  for (const el of marked) {
    el.removeAttribute('data-atx-state');
  }
}

/** 監看 root 的子樹新增節點,把新節點中的可翻區塊交給 onBlocks;回傳停止監看的函式 */
export function watchNewBlocks(
  root: Element,
  opts: CollectOptions,
  onBlocks: (blocks: HTMLElement[]) => void,
): () => void {
  const observer = new MutationObserver((records) => {
    const found = new Set<HTMLElement>();
    for (const r of records) {
      // 已翻好的區塊文字被頁面改掉(例如「載入中…」換成正文):清掉舊譯文重新排入
      const at = r.target.nodeType === Node.ELEMENT_NODE ? (r.target as Element) : r.target.parentElement;
      // stale:之前重翻太頻繁被擋下的區塊,時間窗過後文字再變就恢復重翻
      const done = at && !at.closest(`.${TARGET_CLASS}`) ? at.closest<HTMLElement>('[data-atx-state="done"],[data-atx-state="stale"]') : null;
      if (done && sourceText.has(done) && getBlockText(done) !== sourceText.get(done)) {
        if (resetForRetranslate(done) && isCandidate(done, opts)) found.add(done);
      }
      for (const node of Array.from(r.addedNodes)) {
        if (node.nodeType !== Node.ELEMENT_NODE || !node.isConnected) continue;
        const el = node as Element;
        // 自己插入的譯文不算新段落;主要內容模式下,選單等介面區裡新增的東西也不翻
        if (el.closest(`.${TARGET_CLASS}`) || el.classList.contains(ORIG_CLASS) || inChrome(el, opts)) continue;
        if (el.parentElement && el.parentElement.closest(`[contenteditable]:not([contenteditable="false"])`)) continue;
        const out: HTMLElement[] = [];
        walk(el, opts, out, true);
        out.forEach((b) => found.add(b));
      }
    }
    if (found.size > 0) onBlocks([...found]);
  });
  observer.observe(root, { childList: true, subtree: true, characterData: true });
  return () => observer.disconnect();
}
