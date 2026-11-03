/**
 * YouTube 沒字幕改用聽譯:背景頁對聽譯開一條長連線(connectNative),把字幕事件轉給發起的分頁。
 * 同一時間只有一個分頁在聽(聽譯擷取的是整個瀏覽器的聲音)。開著的 native port 會讓 service worker 保持活著。
 */

export interface PortLike {
  postMessage(m: unknown): void;
  disconnect(): void;
  onMessage: { addListener(f: (m: unknown) => void): void };
  onDisconnect: { addListener(f: () => void): void };
}

const ended = { type: 'listen-event', event: { type: 'ended' } };

export function createListenHub(deps: { connect: () => PortLike; sendToTab: (tabId: number, msg: unknown) => void }) {
  let cur: { tabId: number; port: PortLike } | undefined;

  /** 主動關閉(chrome 不會對自己 disconnect 的 port 觸發 onDisconnect) */
  function drop(): void {
    const c = cur;
    cur = undefined;
    c?.port.disconnect();
  }

  return {
    start(tabId: number): void {
      if (cur?.tabId === tabId) return;
      if (cur) {
        deps.sendToTab(cur.tabId, ended);
        drop();
      }
      const port = deps.connect();
      const me = { tabId, port };
      cur = me;
      port.onMessage.addListener((m) => {
        if (cur === me) deps.sendToTab(tabId, { type: 'listen-event', event: m });
      });
      port.onDisconnect.addListener(() => {
        if (cur !== me) return;
        cur = undefined;
        deps.sendToTab(tabId, ended);
      });
      port.postMessage({ type: 'listen-start' });
    },
    stop(tabId: number): void {
      if (cur?.tabId !== tabId) return;
      cur.port.postMessage({ type: 'listen-stop' });
      drop();
    },
    tabClosed(tabId: number): void {
      if (cur?.tabId === tabId) drop();
    },
    active(): number | undefined {
      return cur?.tabId;
    },
  };
}
