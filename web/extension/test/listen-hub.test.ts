import { describe, expect, it } from 'vitest';
import { createListenHub, type PortLike } from '../src/background/listen.ts';

/** 假 port:記錄送出的訊息,可手動觸發收訊與斷線 */
class FakePort implements PortLike {
  sent: unknown[] = [];
  disconnected = 0;
  private msgHandlers: Array<(m: unknown) => void> = [];
  private dropHandlers: Array<() => void> = [];
  onMessage = { addListener: (f: (m: unknown) => void) => void this.msgHandlers.push(f) };
  onDisconnect = { addListener: (f: () => void) => void this.dropHandlers.push(f) };
  postMessage(m: unknown) {
    this.sent.push(m);
  }
  disconnect() {
    this.disconnected++; // 真的 port 自己斷線不會觸發自己的 onDisconnect
  }
  emit(m: unknown) {
    this.msgHandlers.forEach((f) => f(m));
  }
  drop() {
    this.dropHandlers.forEach((f) => f());
  }
}

function setup() {
  const ports: FakePort[] = [];
  const toTab: Array<{ tabId: number; msg: unknown }> = [];
  const hub = createListenHub({
    connect: () => {
      const p = new FakePort();
      ports.push(p);
      return p;
    },
    sendToTab: (tabId, msg) => void toTab.push({ tabId, msg }),
  });
  const ended = (tabId: number) => ({ tabId, msg: { type: 'listen-event', event: { type: 'ended' } } });
  return { hub, ports, toTab, ended };
}

describe('擴充聽譯連線中樞', () => {
  // U17
  it('開始時連線並送 listen-start', () => {
    const { hub, ports } = setup();
    hub.start(1);
    expect(ports).toHaveLength(1);
    expect(ports[0]!.sent).toEqual([{ type: 'listen-start' }]);
    expect(hub.active()).toBe(1);
  });

  it('同一分頁再開始不重連', () => {
    const { hub, ports } = setup();
    hub.start(1);
    hub.start(1);
    expect(ports).toHaveLength(1); // 重連會讓聽譯被重啟一輪
    expect(hub.active()).toBe(1);
  });

  it('另一分頁開始時舊分頁收到 ended 且舊 port 斷開', () => {
    const { hub, ports, toTab, ended } = setup();
    hub.start(1);
    hub.start(2);
    expect(toTab).toContainEqual(ended(1)); // 沒通知舊分頁,它的字幕會永遠停在聽中
    expect(ports[0]!.disconnected).toBe(1);
    expect(ports[1]!.sent).toEqual([{ type: 'listen-start' }]);
    expect(hub.active()).toBe(2);
  });

  it('port 訊息轉給對應分頁', () => {
    const { hub, ports, toTab } = setup();
    hub.start(1);
    hub.start(2);
    ports[1]!.emit({ type: 'transcript', id: 7 });
    expect(toTab).toContainEqual({ tabId: 2, msg: { type: 'listen-event', event: { type: 'transcript', id: 7 } } });
    expect(toTab.some((t) => t.tabId === 1 && (t.msg as { event: { type: string } }).event.type === 'transcript')).toBe(false);
  });

  it('port 斷線時分頁收到 ended 並清除目前分頁', () => {
    const { hub, ports, toTab, ended } = setup();
    hub.start(2);
    ports[0]!.drop();
    expect(toTab).toContainEqual(ended(2));
    expect(hub.active()).toBeUndefined();
  });

  it('主動 stop 先送 listen-stop 再斷開,不送 ended', () => {
    const { hub, ports, toTab } = setup();
    hub.start(1);
    hub.stop(1);
    expect(ports[0]!.sent).toEqual([{ type: 'listen-start' }, { type: 'listen-stop' }]);
    expect(ports[0]!.disconnected).toBe(1);
    expect(toTab).toEqual([]);
    expect(hub.active()).toBeUndefined();
  });

  it('stop 不是目前分頁時忽略', () => {
    const { hub, ports } = setup();
    hub.start(1);
    hub.stop(9); // 別的分頁關掉不能害目前分頁停止
    expect(ports[0]!.sent).toEqual([{ type: 'listen-start' }]);
    expect(ports[0]!.disconnected).toBe(0);
    expect(hub.active()).toBe(1);
  });

  it('分頁關閉時斷開連線且不送訊息給分頁', () => {
    const { hub, ports, toTab } = setup();
    hub.start(1);
    hub.tabClosed(1);
    expect(ports[0]!.disconnected).toBe(1);
    expect(toTab).toEqual([]);
    expect(hub.active()).toBeUndefined();
  });

  it('關閉的分頁不是目前分頁時忽略', () => {
    const { hub, ports } = setup();
    hub.start(1);
    hub.tabClosed(5);
    expect(ports[0]!.disconnected).toBe(0);
    expect(hub.active()).toBe(1);
  });
});
