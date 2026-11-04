// YouTube 沒字幕改用聽譯:依網站規則、影片有沒有字幕、是否在播放決定要聽譯開始或停止,
// 聽譯送回的字幕畫在播放器裡(#movie_player 內,全螢幕也看得到)。
import type { SiteRule } from '../shared/sites.ts';
import { listenAction } from '../shared/youtube.ts';
import { mountLiveBox } from './liveBox.ts';
import { initialLive, liveView, reduceLive, type LiveState } from './liveSubs.ts';

export function createYouTubeListen() {
  let rule: SiteRule | undefined;
  let info: { videoId: string | null; hasCaptions: boolean | null } = { videoId: null, hasCaptions: null };
  let listening = false;
  let live: LiveState = initialLive();
  /** 字幕框上方的小字:啟動中、使用中、錯誤 */
  let note = '';
  /** 聽譯回「使用中」或「無法開始」的那支影片:提示留著、這支不再自動重試,換影片才再試 */
  let blockedFor: string | null = null;

  const playing = () => {
    const v = document.querySelector<HTMLVideoElement>('#movie_player video');
    return !!v && !v.paused && !v.ended;
  };

  let box: ReturnType<typeof mountLiveBox> | null = null;

  function render(): void {
    const player = document.getElementById('movie_player');
    const view = liveView(live);
    // 沒在聽時只留「使用中/無法開始」的提示
    const showNote = !listening && blockedFor !== null && blockedFor === info.videoId;
    if ((!listening && !showNote) || !player || (!view && !note)) {
      box?.remove();
      box = null;
      return;
    }
    // YouTube 換影片時可能重建播放器內容:字幕框不在播放器裡就重新掛
    if (!box || box.host.parentElement !== player) box = mountLiveBox(player);
    const rows: { cls: string; text: string }[] = [];
    if (note) rows.push({ cls: 'atx-live-note', text: note });
    if (view?.text) rows.push({ cls: `atx-live-text${view.pending ? ' pending' : ''}`, text: view.text });
    if (view?.partial) rows.push({ cls: 'atx-live-partial', text: view.partial });
    box.render(rows);
  }

  function reconsider(): void {
    const act = listenAction({ rule, hasCaptions: info.hasCaptions, playing: playing(), onWatchPage: location.pathname === '/watch', listening });
    if (act === 'start' && blockedFor !== null && blockedFor === info.videoId) {
      // 主視窗正在用聽譯或開不起來:這支影片不重試
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
        // 使用中/無法開始的提示留著(連線隨即關閉也會送 ended),其他狀態清掉
        if (blockedFor !== info.videoId) note = '';
      } else if (ev.type === 'listen_status') {
        const msg = typeof ev.message === 'string' ? ev.message : '';
        blockedFor = ev.state === 'busy' || ev.state === 'error' ? info.videoId : null;
        note = ev.state === 'starting' ? '聽譯啟動中…' : ev.state === 'busy' ? `${msg},這支影片不會自動聽譯` : ev.state === 'error' ? `聽譯無法開始:${msg}` : '';
      } else {
        live = reduceLive(live, ev);
      }
      render();
    },
  };
}
