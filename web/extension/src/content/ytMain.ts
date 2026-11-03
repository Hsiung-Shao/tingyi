// YouTube 頁面世界(MAIN world):content script 讀不到播放器物件,這裡讀「這支影片有沒有字幕」,
// 用 window 事件轉告 content script。物件不能跨 world,detail 用 JSON 字串。
import { hasCaptions } from '../shared/youtube.ts';

interface Player extends HTMLElement {
  getPlayerResponse?: () => unknown;
}

let last = '';

function report(): void {
  const player = document.getElementById('movie_player') as Player | null;
  const res = player?.getPlayerResponse?.() as { videoDetails?: { videoId?: string } } | undefined;
  const detail = JSON.stringify({
    videoId: res?.videoDetails?.videoId ?? null,
    // 播放器還沒載入影片資料時回 null(還不知道),不當成沒字幕
    hasCaptions: res ? hasCaptions(res) : null,
  });
  if (detail === last) return;
  last = detail;
  window.dispatchEvent(new CustomEvent('atx-yt', { detail }));
}

// YouTube 是單頁應用:換影片時沒有頁面載入,靠它自己的導覽事件與 video 的 loadedmetadata
document.addEventListener('yt-navigate-finish', report);
document.addEventListener('loadedmetadata', report, true);
// content script 晚於這裡載入時會要一次目前狀態
window.addEventListener('atx-yt-request', () => {
  last = '';
  report();
});
setInterval(report, 2000);
report();
