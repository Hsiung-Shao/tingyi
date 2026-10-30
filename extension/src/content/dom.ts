import { isCodeLike, isLikelyTargetLang } from '@atx/core';
import type { DisplayMode, Scope } from '../shared/settings.ts';

export const TARGET_CLASS = 'atx-target';
/** 短標籤的譯文同行顯示時加的 class */
export const INLINE_CLASS = 'atx-inline';

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

/** only:在 <html> 設 data-atx-mode="only"(CSS 隱藏已翻段落的原文);dual:移除該屬性 */
export function setDisplayMode(mode: DisplayMode, doc: Document = document): void {
  if (mode === 'only') doc.documentElement.dataset.atxMode = 'only';
  else delete doc.documentElement.dataset.atxMode;
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
  el.querySelector(`:scope > font.${TARGET_CLASS}`)?.remove();
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
  if (el.querySelector(`:scope > font.${TARGET_CLASS}`)) return false;
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
 * source 是送翻時的原文;插入時原文已經不同(翻譯途中被頁面改掉)就回 true,表示譯文已過期。
 */
export function insertTranslation(el: HTMLElement, text: string, source?: string): boolean {
  let target = el.querySelector<HTMLElement>(`:scope > font.${TARGET_CLASS}`);
  const current = getBlockText(el);
  if (!target) {
    target = el.ownerDocument.createElement('font');
    target.className = TARGET_CLASS;
    // 選單項目、按鈕這類短標籤:譯文接在同一行,不另起一行撐開版面
    if (!/^H[1-6]$/.test(el.tagName.toUpperCase()) && current.length <= INLINE_MAX_CHARS) target.classList.add(INLINE_CLASS);
    const firstBlock = Array.from(el.children).find((c) => c.matches(BLOCKISH_SELECTOR));
    el.insertBefore(target, firstBlock ?? null);
  }
  // 僅譯文模式會把區塊字級設 0,譯文要用原本的字級;已經是 0(模式切換後重插)就不覆寫
  const fs = el.ownerDocument.defaultView?.getComputedStyle(el).fontSize ?? '';
  if (parseFloat(fs) > 0) target.style.setProperty('--atx-fs', fs);
  target.textContent = text;
  sourceText.set(el, source ?? current);
  return source !== undefined && source !== current;
}

/** 移除 root 底下所有譯文與 data-atx-state 標記,還原頁面 */
export function removeAllTranslations(root: Element): void {
  root.querySelectorAll(`font.${TARGET_CLASS}`).forEach((f) => f.remove());
  const marked = [root, ...Array.from(root.querySelectorAll('[data-atx-state]'))];
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
        if (el.closest(`.${TARGET_CLASS}`) || inChrome(el, opts)) continue;
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
