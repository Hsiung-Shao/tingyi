// YouTube 沒字幕改用聽譯:依網站規則、影片有沒有字幕、是否在播放決定要聽譯開始或停止,
// 聽譯送回的字幕畫在播放器裡(#movie_player 內,全螢幕也看得到)。
import type { SiteRule } from '../shared/sites.ts';
import { listenAction } from '../shared/youtube.ts';
import { initialLive, liveView, reduceLive, type LiveState } from './liveSubs.ts';

const BOX_ID = 'atx-live';

export function createYouTubeListen() {
  let rule: SiteRule | undefined;
  let info: { videoId: string | null; hasCaptions: boolean | null } = { videoId: null, hasCaptions: null };
  let listening = false;
  let live: LiveState = initialLive();
  /** 字幕框上方的小字:啟動中、使用中、錯誤 */
  let note = '';
  /** 聽譯回「使用中」的那支影片:這支不再自動重試,換影片才再試 */
  let busyFor: string | null = null;

  const playing = () => {
    const v = document.querySelector<HTMLVideoElement>('#movie_player video');
    return !!v && !v.paused && !v.ended;
  };

  function render(): void {
    const player = document.getElementById('movie_player');
    let box = document.getElementById(BOX_ID);
    const view = liveView(live);
    // 沒在聽時只留「使用中」的提示
    const showBusy = !listening && busyFor !== null && busyFor === info.videoId;
    if ((!listening && !showBusy) || !player || (!view && !note)) {
      box?.remove();
      return;
    }
    if (!box || box.parentElement !== player) {
      box?.remove();
      box = document.createElement('div');
      box.id = BOX_ID;
      box.className = 'atx-live';
      player.appendChild(box);
    }
    const rows: HTMLElement[] = [];
    const row = (cls: string, text: string) => {
      const el = document.createElement('div');
      el.className = cls;
      // 只用 textContent:字幕內容來自語音辨識,不當成 HTML
      el.textContent = text;
      rows.push(el);
    };
    if (note) row('atx-live-note', note);
    if (view?.text) row(`atx-live-text${view.pending ? ' pending' : ''}`, view.text);
    if (view?.partial) row('atx-live-partial', view.partial);
    box.replaceChildren(...rows);
  }

  function reconsider(): void {
    const act = listenAction({ rule, hasCaptions: info.hasCaptions, playing: playing(), onWatchPage: location.pathname === '/watch', listening });
    if (act === 'start' && busyFor !== null && busyFor === info.videoId) {
      // 主視窗正在用聽譯:這支影片不重試
    } else if (act === 'start') {
      listening = true;
      live = initialLive();
      note = '聽譯啟動中…';
      void chrome.runtime.sendMessage({ type: 'listen-start' }).catch(() => {});
    } else if (act === 'stop') {
      listening = false;
      live = initialLive();
      note = '';
      void chrome.runtime.sendMessage({ type: 'listen-stop' }).catch(() => {});
    }
    render();
  }

  window.addEventListener('atx-yt', (e) => {
    const next = JSON.parse((e as CustomEvent<string>).detail) as typeof info;
    // 換影片:清掉上一支的字幕與提示
    if (next.videoId !== info.videoId) {
      live = initialLive();
      if (!listening) note = '';
    }
    info = next;
    reconsider();
  });
  document.addEventListener('play', reconsider, true);
  document.addEventListener('pause', reconsider, true);
  // 頁面世界的腳本可能比這裡先載入:要一次目前狀態
  window.dispatchEvent(new Event('atx-yt-request'));

  return {
    setRule(next: SiteRule | undefined): void {
      rule = next;
      reconsider();
    },
    onEvent(ev: { type: string; [k: string]: unknown }): void {
      if (ev.type === 'ended') {
        listening = false;
        live = initialLive();
        // 使用中的提示留著(連線隨即關閉也會送 ended),其他狀態清掉
        if (busyFor !== info.videoId) note = '';
      } else if (ev.type === 'listen_status') {
        const msg = typeof ev.message === 'string' ? ev.message : '';
        busyFor = ev.state === 'busy' ? info.videoId : null;
        note = ev.state === 'starting' ? '聽譯啟動中…' : ev.state === 'busy' ? `${msg},這支影片不會自動聽譯` : ev.state === 'error' ? `聽譯無法開始:${msg}` : '';
      } else {
        live = reduceLive(live, ev);
      }
      render();
    },
  };
}
