// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { mountLiveBox } from '../src/content/liveBox.ts';

const ROWS = [
  { cls: 'tr', text: '中文字幕' },
  { cls: 'partial', text: 'preview' },
];

function makePlayer(): HTMLElement {
  const player = document.createElement('div');
  player.id = 'movie_player';
  document.body.appendChild(player);
  return player;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('播放器字幕框', () => {
  // P1
  it('字幕放在頁面讀不到的 closed shadow root', () => {
    const player = makePlayer();
    const box = mountLiveBox(player);
    box.render(ROWS);

    const found = player.querySelector('#atx-live');
    expect(found).not.toBeNull();
    expect(found).toBe(box.host);
    expect(box.host.shadowRoot).toBeNull(); // open 的話頁面腳本能讀到字幕內容
    for (const text of ['中文字幕', 'preview']) {
      expect(player.textContent).not.toContain(text);
      expect(document.body.textContent).not.toContain(text);
      expect(document.body.innerHTML).not.toContain(text); // 寫進 light DOM 的屬性或子節點也會被抓到
      expect(document.body.innerText ?? '').not.toContain(text);
    }
  });

  // P2
  it('重複 render 不影響頁面層、remove 後從 DOM 移除', () => {
    const player = makePlayer();
    const box = mountLiveBox(player);
    box.render(ROWS);
    box.render([{ cls: 'tr', text: '第二批' }]);
    box.render([]);
    expect(player.querySelectorAll('#atx-live')).toHaveLength(1);
    expect(player.textContent).not.toContain('第二批');

    box.remove();
    expect(player.querySelector('#atx-live')).toBeNull();
    expect(document.getElementById('atx-live')).toBeNull();
  });

  it('同一播放器再掛載不會出現兩個字幕框', () => {
    const player = makePlayer();
    const a = mountLiveBox(player);
    a.render(ROWS);
    const b = mountLiveBox(player);
    b.render(ROWS);
    expect(player.querySelectorAll('#atx-live')).toHaveLength(1); // 重複掛載會疊出兩層字幕
    b.remove();
    expect(player.querySelectorAll('#atx-live')).toHaveLength(0);
  });
});
