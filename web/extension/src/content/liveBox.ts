// 播放器裡的聽譯字幕框。字幕來自整個瀏覽器的聲音(可能含其他分頁的對話),
// 放在 closed shadow root 裡,YouTube 與頁面上的其他腳本讀不到內容;樣式也只在 shadow root 內生效。

const HOST_ID = 'atx-live';

const CSS = `
:host { all: initial; }
.box { text-align: center; font-family: 'Microsoft JhengHei', 'PingFang TC', sans-serif; }
.atx-live-text {
  display: inline-block;
  padding: 2px 10px;
  background: rgba(8, 8, 8, 0.75);
  color: #fff;
  font-size: clamp(16px, 2.6vw, 36px);
  line-height: 1.3;
}
.atx-live-text.pending { color: #bdbdbd; }
.atx-live-partial {
  margin-top: 4px;
  color: #e0e0e0;
  font-size: clamp(12px, 1.6vw, 22px);
  font-style: italic;
  text-shadow: 0 0 4px #000, 0 0 2px #000;
}
.atx-live-note {
  display: inline-block;
  margin-bottom: 4px;
  padding: 1px 8px;
  background: rgba(0, 0, 0, 0.6);
  color: #ddd;
  font-size: 12px;
}
`;

export function mountLiveBox(player: HTMLElement) {
  player.querySelector(`#${HOST_ID}`)?.remove();
  const host = player.ownerDocument.createElement('div');
  host.id = HOST_ID;
  host.className = 'atx-live';
  const root = host.attachShadow({ mode: 'closed' });
  const style = player.ownerDocument.createElement('style');
  style.textContent = CSS;
  const box = player.ownerDocument.createElement('div');
  box.className = 'box';
  root.append(style, box);
  player.appendChild(host);
  return {
    host,
    render(rows: { cls: string; text: string }[]): void {
      box.replaceChildren(
        ...rows.map((r) => {
          const el = player.ownerDocument.createElement('div');
          el.className = r.cls;
          // 只用 textContent:字幕內容來自語音辨識,不當成 HTML
          el.textContent = r.text;
          return el;
        }),
      );
    },
    remove(): void {
      host.remove();
    },
  };
}
