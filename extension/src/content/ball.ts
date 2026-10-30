import type { TabState } from '../shared/messages.ts';
import type { DisplayMode, Scope } from '../shared/settings.ts';

export interface BallPrefs {
  displayMode: DisplayMode;
  scope: Scope;
}

export interface BallOptions {
  onToggle(): void;
  onMode(mode: DisplayMode): void;
  onScope(scope: Scope): void;
  onOpenOptions(): void;
  /** 拖曳結束時回報新的 top(px),由呼叫端存起來 */
  onMove?(top: number): void;
  initial: { state: TabState; prefs: BallPrefs; top?: number };
  /** 只給 jsdom 測試用:程式派發的事件 isTrusted 為 false,正式環境一律拒絕 */
  allowSyntheticEvents?: boolean;
}

export interface BallHandle {
  /** 掛在 document.documentElement 底下、帶 open shadowRoot 的 div */
  host: HTMLElement;
  update(state: TabState, prefs: BallPrefs): void;
  destroy(): void;
}

const DRAG_THRESHOLD_PX = 4;
/** 浮動球與面板只會出現在視窗右側這個寬度內(面板 236 + 球 48 + 邊距) */
const RIGHT_ZONE_PX = 340;

/**
 * 防點擊劫持:網頁可以改 host(在 light DOM)或 open shadow 內元素的樣式,把按鈕放大成全畫面透明,
 * 讓使用者的下一次真實點擊落在按鈕上。被點的元素必須尺寸正常、在右側區域、自己到 host 都不透明可見,
 * host 也不能被 transform;不符就不算數。
 */
export function isGenuineTarget(el: Element, host: HTMLElement, win: Window): boolean {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0 || r.width > 320 || r.height > 420) return false;
  if (r.left < win.innerWidth - RIGHT_ZONE_PX || r.top < -4 || r.bottom > win.innerHeight + 4) return false;
  for (let n: Element | null = el; n; ) {
    const cs = win.getComputedStyle(n);
    const opacity = parseFloat(cs.opacity);
    if ((!Number.isNaN(opacity) && opacity < 0.95) || cs.visibility === 'hidden' || cs.display === 'none') return false;
    // filter: opacity(0)、clip-path 也能把按鈕藏起來
    if ((cs.filter && cs.filter !== 'none') || (cs.clipPath && cs.clipPath !== 'none')) return false;
    if (n === host) return !cs.transform || cs.transform === 'none';
    n = n.parentElement ?? ((n.getRootNode() as ShadowRoot).host as Element | undefined) ?? null;
  }
  return false;
}

const STYLE = `
:host { all: initial; }
* { box-sizing: border-box; }
.wrap {
  --accent: #4f5bd5; --bg: #ffffff; --fg: #1d2330; --muted: #667085; --line: #e4e7ec; --seg: #f2f4f7; --bad: #c0392b;
  position: relative; font: 13px/1.45 system-ui, 'Segoe UI', 'Microsoft JhengHei', sans-serif; color: var(--fg);
}
@media (prefers-color-scheme: dark) {
  .wrap { --accent: #8b94ff; --bg: #1c1f27; --fg: #e6e8ee; --muted: #98a1b3; --line: #2f3542; --seg: #262a34; --bad: #ff7a6b; }
}
.dock {
  display: flex; flex-direction: column; align-items: center; gap: 4px;
  transform: translateX(14px); transition: transform .18s ease;
}
.wrap:hover .dock, .wrap.on .dock, .wrap.open .dock, .dock:focus-within { transform: translateX(0); }
button { font: inherit; color: inherit; cursor: pointer; border: 0; background: none; padding: 0; }
button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.ball {
  position: relative; width: 40px; height: 40px; border-radius: 20px 0 0 20px;
  display: grid; place-items: center; touch-action: none; user-select: none;
  background: var(--bg); color: var(--muted); border: 1px solid var(--line); border-right: 0;
  box-shadow: 0 2px 10px rgba(16, 24, 40, .16); font-weight: 700; font-size: 16px;
}
.ball[aria-pressed='true'] { background: var(--accent); color: #fff; border-color: var(--accent); }
.ball.busy::after {
  content: ''; position: absolute; inset: 3px; border-radius: 50%;
  border: 2px solid rgba(255,255,255,.85); border-top-color: transparent; animation: spin 1s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }
.more {
  width: 22px; height: 22px; border-radius: 11px 0 0 11px; display: grid; place-items: center;
  background: var(--bg); color: var(--muted); border: 1px solid var(--line); border-right: 0; font-size: 12px;
  margin-left: 18px;
}
.more[aria-expanded='true'] { color: var(--accent); }
.panel {
  display: none; position: absolute; right: 48px; top: 0; width: 236px; padding: 12px;
  background: var(--bg); border: 1px solid var(--line); border-radius: 12px;
  box-shadow: 0 12px 32px rgba(16, 24, 40, .22);
}
.wrap.open .panel { display: block; }
.wrap.up .panel { top: auto; bottom: 0; }
.head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 10px; }
.title { font-weight: 650; font-size: 13px; }
.switch {
  width: 36px; height: 20px; border-radius: 10px; background: var(--line); position: relative; transition: background .15s;
}
.switch::after {
  content: ''; position: absolute; top: 2px; left: 2px; width: 16px; height: 16px; border-radius: 50%;
  background: #fff; box-shadow: 0 1px 2px rgba(0,0,0,.25); transition: transform .15s;
}
.switch[aria-pressed='true'] { background: var(--accent); }
.switch[aria-pressed='true']::after { transform: translateX(16px); }
.label { color: var(--muted); font-size: 12px; margin: 10px 0 4px; }
.seg { display: flex; background: var(--seg); border-radius: 8px; padding: 2px; }
.seg button { flex: 1; padding: 5px 0; border-radius: 6px; color: var(--muted); }
.seg button[aria-pressed='true'] { background: var(--bg); color: var(--fg); font-weight: 600; box-shadow: 0 1px 2px rgba(16,24,40,.12); }
.status { margin-top: 10px; color: var(--muted); font-size: 12px; min-height: 1.45em; word-break: break-word; white-space: pre-line; }
.status.bad { color: var(--bad); }
.foot { margin-top: 8px; padding-top: 8px; border-top: 1px solid var(--line); }
.link { color: var(--accent); font-size: 12px; }
`;

function seg(doc: Document, name: string, label: string): HTMLButtonElement {
  const b = doc.createElement('button');
  b.type = 'button';
  b.dataset.atx = name;
  b.textContent = label;
  return b;
}

/**
 * 右緣浮動球 + 滑出面板。shadowRoot 內的元素用 data-atx 屬性定位:
 * - [data-atx="toggle"]:主按鈕,點擊 → onToggle;aria-pressed 反映 state.enabled
 * - [data-atx="expand"]:展開/收合面板的按鈕
 * - [data-atx="panel"]:面板(收合時不可見)
 * - [data-atx="mode-dual"] / [data-atx="mode-only"]:顯示模式,點擊 → onMode;aria-pressed 反映 prefs.displayMode
 * - [data-atx="scope-main"] / [data-atx="scope-page"]:翻譯範圍,點擊 → onScope;aria-pressed 反映 prefs.scope
 * - [data-atx="status"]:狀態文字,含已翻段數(state.done)與錯誤
 * - [data-atx="options"]:進階設定連結 → onOpenOptions
 * 主按鈕上 pointerdown 後移動超過 4px 視為拖曳:改 host.style.top、結束時呼叫 onMove,且不觸發 onToggle。
 */
export function createBall(doc: Document, opts: BallOptions): BallHandle {
  const host = doc.createElement('div');
  host.dataset.atxHost = '';
  host.setAttribute('translate', 'no');
  // inline + important:網站的全域 div 樣式蓋不掉定位
  const fix = (prop: string, value: string) => host.style.setProperty(prop, value, 'important');
  // 存下來的位置可能來自更大的螢幕:夾在目前視窗內,球才看得到、拖得回來
  const clampTop = (top: number) => Math.min((doc.defaultView?.innerHeight ?? 800) - 48, Math.max(8, top));
  fix('position', 'fixed');
  fix('right', '0');
  fix('z-index', '2147483646');
  fix('display', 'block');
  fix('top', `${clampTop(opts.initial.top ?? Math.round((doc.defaultView?.innerHeight ?? 800) * 0.45))}px`);
  const root = host.attachShadow({ mode: 'open' });

  const style = doc.createElement('style');
  style.textContent = STYLE;
  const wrap = doc.createElement('div');
  wrap.className = 'wrap';

  const dock = doc.createElement('div');
  dock.className = 'dock';
  const ball = doc.createElement('button');
  ball.type = 'button';
  ball.className = 'ball';
  ball.dataset.atx = 'toggle';
  ball.textContent = '譯';
  const more = doc.createElement('button');
  more.type = 'button';
  more.className = 'more';
  more.dataset.atx = 'expand';
  more.setAttribute('aria-label', '翻譯設定');
  more.setAttribute('aria-expanded', 'false');
  more.textContent = '‹';
  dock.append(ball, more);

  const panel = doc.createElement('div');
  panel.className = 'panel';
  panel.dataset.atx = 'panel';
  panel.setAttribute('role', 'dialog');
  panel.setAttribute('aria-label', 'Agent Translator 快速設定');
  const head = doc.createElement('div');
  head.className = 'head';
  const title = doc.createElement('span');
  title.className = 'title';
  title.textContent = '翻譯此頁';
  const sw = doc.createElement('button');
  sw.type = 'button';
  sw.className = 'switch';
  sw.dataset.atx = 'panel-toggle';
  sw.setAttribute('aria-label', '翻譯此頁');
  head.append(title, sw);

  const modeLabel = doc.createElement('div');
  modeLabel.className = 'label';
  modeLabel.textContent = '顯示';
  const modeSeg = doc.createElement('div');
  modeSeg.className = 'seg';
  const modeDual = seg(doc, 'mode-dual', '雙語對照');
  const modeOnly = seg(doc, 'mode-only', '僅譯文');
  modeSeg.append(modeDual, modeOnly);

  const scopeLabel = doc.createElement('div');
  scopeLabel.className = 'label';
  scopeLabel.textContent = '範圍';
  const scopeSeg = doc.createElement('div');
  scopeSeg.className = 'seg';
  const scopeMain = seg(doc, 'scope-main', '主要內容');
  const scopePage = seg(doc, 'scope-page', '整頁');
  scopeSeg.append(scopeMain, scopePage);

  const status = doc.createElement('div');
  status.className = 'status';
  status.dataset.atx = 'status';
  status.setAttribute('aria-live', 'polite');
  const foot = doc.createElement('div');
  foot.className = 'foot';
  const optionsBtn = doc.createElement('button');
  optionsBtn.type = 'button';
  optionsBtn.className = 'link';
  optionsBtn.dataset.atx = 'options';
  optionsBtn.textContent = '進階設定';
  foot.append(optionsBtn);

  panel.append(head, modeLabel, modeSeg, scopeLabel, scopeSeg, status, foot);
  wrap.append(panel, dock);
  root.append(style, wrap);
  doc.documentElement.appendChild(host);

  // 浮動球只接受滑鼠/觸控,按鈕不放進 Tab 順序(鍵盤使用者用 Alt+A 或 popup)
  root.querySelectorAll('button').forEach((b) => (b.tabIndex = -1));

  const setOpen = (open: boolean) => {
    wrap.classList.toggle('open', open);
    more.setAttribute('aria-expanded', String(open));
    // 球太靠下方時面板往上展開,整個面板留在視窗內
    if (open) {
      const room = (doc.defaultView?.innerHeight ?? 800) - host.getBoundingClientRect().top;
      wrap.classList.toggle('up', panel.offsetHeight > room - 8);
    }
  };

  /**
   * 只接受使用者本人的操作:shadow root 是 open,網頁程式可以 click() 代按,
   * 自動開翻譯就等於把頁面內容送進使用者的訂閱 agent,也能改掉全域設定。
   */
  const win = doc.defaultView ?? window;
  const trusted = (e: Event) => e.isTrusted || opts.allowSyntheticEvents === true;
  /*
   * 點擊與按下時另外檢查按鈕沒被網頁竄改(拖曳中的移動與放開不檢查)。
   * 只接受滑鼠/觸控點擊(detail > 0):鍵盤觸發的 click 會被網頁搬焦點利用,
   * 滑鼠點完焦點留在球上時按空白鍵捲頁也會誤關翻譯。鍵盤使用者改用 Alt+A 或 popup(使用者決定)。
   */
  const genuine = (el: HTMLElement, e: Event) => {
    if (opts.allowSyntheticEvents === true || (e.type !== 'click' && e.type !== 'pointerdown')) return true;
    if (!isGenuineTarget(el, host, win)) return false;
    if (e.type === 'click' && (!(e instanceof MouseEvent) || e.detail === 0)) return false;
    // 滑鼠點擊位置最上層真的是浮動球
    if (e instanceof MouseEvent && e.type === 'click' && doc.elementFromPoint(e.clientX, e.clientY) !== host) return false;
    return true;
  };
  const on = (el: HTMLElement, type: string, fn: (e: PointerEvent) => void) =>
    el.addEventListener(type, (e) => {
      if (trusted(e) && genuine(el, e)) fn(e as PointerEvent);
    });

  // 拖曳:pointerdown 記起點,移動超過門檻才算拖曳;拖曳後的 click 不算開關
  let dragStart: { y: number; top: number } | null = null;
  let dragged = false;
  on(ball, 'pointerdown', (e) => {
    dragStart = { y: e.clientY, top: parseFloat(host.style.top) || 0 };
    dragged = false;
    ball.setPointerCapture?.(e.pointerId);
  });
  on(ball, 'pointermove', (e) => {
    if (!dragStart) return;
    const dy = e.clientY - dragStart.y;
    if (!dragged && Math.abs(dy) <= DRAG_THRESHOLD_PX) return;
    dragged = true;
    fix('top', `${clampTop(dragStart.top + dy)}px`);
  });
  const endDrag = () => {
    if (dragStart && dragged) opts.onMove?.(parseFloat(host.style.top) || 0);
    dragStart = null;
  };
  on(ball, 'pointerup', endDrag);
  on(ball, 'pointercancel', endDrag);
  on(ball, 'click', () => {
    if (dragged) {
      dragged = false;
      return;
    }
    opts.onToggle();
  });

  on(sw, 'click', () => opts.onToggle());
  on(more, 'click', () => setOpen(!wrap.classList.contains('open')));
  on(modeDual, 'click', () => opts.onMode('dual'));
  on(modeOnly, 'click', () => opts.onMode('only'));
  on(scopeMain, 'click', () => opts.onScope('main'));
  on(scopePage, 'click', () => opts.onScope('page'));
  on(optionsBtn, 'click', () => opts.onOpenOptions());

  // 點面板外或按 Esc 收合
  const onDocDown = (e: Event) => {
    if (wrap.classList.contains('open') && !e.composedPath().includes(host)) setOpen(false);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') setOpen(false);
  };
  // 視窗縮小時把球夾回可見範圍
  const onResize = () => fix('top', `${clampTop(parseFloat(host.style.top) || 0)}px`);
  doc.addEventListener('pointerdown', onDocDown, true);
  doc.addEventListener('keydown', onKey, true);
  win.addEventListener('resize', onResize);

  const update = (state: TabState, prefs: BallPrefs) => {
    const on = String(state.enabled);
    ball.setAttribute('aria-pressed', on);
    sw.setAttribute('aria-pressed', on);
    ball.setAttribute('aria-label', state.enabled ? '關閉翻譯(顯示原文)' : '翻譯此頁');
    ball.classList.toggle('busy', state.enabled && state.pending > 0);
    wrap.classList.toggle('on', state.enabled);
    modeDual.setAttribute('aria-pressed', String(prefs.displayMode === 'dual'));
    modeOnly.setAttribute('aria-pressed', String(prefs.displayMode === 'only'));
    scopeMain.setAttribute('aria-pressed', String(prefs.scope === 'main'));
    scopePage.setAttribute('aria-pressed', String(prefs.scope === 'page'));
    if (!state.enabled) status.textContent = '尚未翻譯';
    // 錯誤細節可能含本機路徑;open shadow root 頁面讀得到,所以這裡只給段數,細節看 popup
    else status.textContent = `已翻 ${state.done} 段${state.pending ? `,翻譯中 ${state.pending} 段` : ''}${state.errors ? `\n${state.errors} 段失敗,原因請看擴充圖示的視窗` : ''}`;
    status.classList.toggle('bad', state.errors > 0);
  };
  update(opts.initial.state, opts.initial.prefs);

  return {
    host,
    update,
    destroy() {
      doc.removeEventListener('pointerdown', onDocDown, true);
      doc.removeEventListener('keydown', onKey, true);
      win.removeEventListener('resize', onResize);
      host.remove();
    },
  };
}
