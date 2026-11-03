import type { SiteRule } from './sites.ts';

/** 播放器回應裡有沒有字幕軌;YouTube 自動產生的字幕(kind 'asr')也算 */
export function hasCaptions(playerResponse: unknown): boolean {
  const tracks = (playerResponse as { captions?: { playerCaptionsTracklistRenderer?: { captionTracks?: unknown } } } | null)
    ?.captions?.playerCaptionsTracklistRenderer?.captionTracks;
  return Array.isArray(tracks) && tracks.length > 0;
}

export type ListenAction = 'start' | 'stop' | 'none';

/**
 * 要不要讓聽譯開始或停止聽這支影片:網站規則開了「沒字幕時用聽譯」、在觀看頁、確定沒有字幕才要聽;
 * 播放中才開始,暫停時維持(聽譯沒聲音會自動收起)。hasCaptions 為 null 表示還不知道,不算沒字幕。
 */
export function listenAction(i: { rule: SiteRule | undefined; hasCaptions: boolean | null; playing: boolean; onWatchPage: boolean; listening: boolean }): ListenAction {
  const want = i.rule?.listen === true && i.onWatchPage && i.hasCaptions === false;
  if (want && i.playing && !i.listening) return 'start';
  if (i.listening && !want) return 'stop';
  return 'none';
}
