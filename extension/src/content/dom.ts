import { isLikelyTargetLang } from '@atx/core';

export const TARGET_CLASS = 'atx-target';

export interface CollectOptions {
  targetLang: string;
  /** 段落去空白後最少字元數 */
  minChars: number;
}

/** 本身就是一段文字的區塊 */
const LEAF_TAGS = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'LI', 'TD', 'TH', 'BLOCKQUOTE', 'DD', 'DT', 'FIGCAPTION', 'SUMMARY', 'CAPTION', 'LEGEND']);
/** 容器:裡面沒有其他區塊時才當成一段 */
const CONTAINER_TAGS = new Set(['DIV', 'SECTION', 'ARTICLE', 'MAIN', 'ASIDE', 'HEADER', 'FOOTER', 'NAV', 'FIGURE', 'DETAILS', 'LABEL']);
/** 整個子樹都不翻 */
const SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'PRE', 'CODE', 'KBD', 'SAMP', 'TEXTAREA', 'INPUT', 'SELECT', 'OPTION', 'BUTTON', 'SVG', 'MATH', 'CANVAS', 'VIDEO', 'AUDIO', 'IFRAME', 'OBJECT', 'HEAD']);
/** 判斷「裡面還有沒有區塊」用:多了 pre、ul、table 這類不翻但會斷段的元素 */
const BLOCKISH_SELECTOR = [...LEAF_TAGS, ...CONTAINER_TAGS, 'PRE', 'UL', 'OL', 'DL', 'TABLE', 'TR', 'HR']
  .map((t) => t.toLowerCase())
  .join(',');

function isSkipped(el: Element): boolean {
  if (SKIP_TAGS.has(el.tagName.toUpperCase())) return true;
  if (el.classList.contains(TARGET_CLASS) || el.classList.contains('notranslate')) return true;
  if (el.getAttribute('translate') === 'no') return true;
  const ce = el.getAttribute('contenteditable');
  return ce !== null && ce !== 'false';
}

/** 插入譯文當下的原文;文字被頁面改掉時用來判斷要不要重翻 */
const sourceText = new WeakMap<Element, string>();

function isCandidate(el: Element, opts: CollectOptions): el is HTMLElement {
  const tag = el.tagName.toUpperCase();
  if (!LEAF_TAGS.has(tag) && !CONTAINER_TAGS.has(tag)) return false;
  if ((el as HTMLElement).dataset?.atxState !== undefined) return false;
  if (el.querySelector(`:scope > font.${TARGET_CLASS}`)) return false;
  const text = getBlockText(el);
  if (text.length < opts.minChars) return false;
  // 全漢字的日文標題看起來像中文,有 lang 標示就以它為準
  if (el.closest('[lang]')?.getAttribute('lang')?.toLowerCase().startsWith('ja')) return true;
  return !isLikelyTargetLang(text, opts.targetLang);
}

function walk(el: Element, opts: CollectOptions, out: HTMLElement[], includeSelf: boolean): void {
  if (isSkipped(el)) return;
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
        if (p.classList.contains(TARGET_CLASS) || p.tagName === 'SCRIPT' || p.tagName === 'STYLE') return NodeFilter.FILTER_REJECT;
        if (p.matches(BLOCKISH_SELECTOR) || SKIP_TAGS.has(p.tagName.toUpperCase())) return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  let text = '';
  for (let n = walker.nextNode(); n; n = walker.nextNode()) text += n.nodeValue ?? '';
  return text.replace(/\s+/g, ' ').trim();
}

/** 在區塊內插入 <font class="atx-target">:一般放最後面,有子區塊時放在第一個子區塊前;已存在就只更新文字 */
export function insertTranslation(el: HTMLElement, text: string): void {
  let target = el.querySelector<HTMLElement>(`:scope > font.${TARGET_CLASS}`);
  if (!target) {
    target = el.ownerDocument.createElement('font');
    target.className = TARGET_CLASS;
    const firstBlock = Array.from(el.children).find((c) => c.matches(BLOCKISH_SELECTOR));
    el.insertBefore(target, firstBlock ?? null);
  }
  target.textContent = text;
  sourceText.set(el, getBlockText(el));
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
      const done = at && !at.closest(`.${TARGET_CLASS}`) ? at.closest<HTMLElement>('[data-atx-state="done"]') : null;
      if (done && sourceText.has(done) && getBlockText(done) !== sourceText.get(done)) {
        done.querySelector(`:scope > font.${TARGET_CLASS}`)?.remove();
        delete done.dataset.atxState;
        sourceText.delete(done);
        if (isCandidate(done, opts)) found.add(done);
      }
      for (const node of Array.from(r.addedNodes)) {
        if (node.nodeType !== Node.ELEMENT_NODE || !node.isConnected) continue;
        const el = node as Element;
        // 自己插入的譯文不算新段落
        if (el.closest(`.${TARGET_CLASS}`)) continue;
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
