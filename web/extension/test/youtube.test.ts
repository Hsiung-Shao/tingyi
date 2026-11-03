import { describe, expect, it } from 'vitest';
import { hasCaptions, listenAction } from '../src/shared/youtube.ts';
import type { SiteRule } from '../src/shared/sites.ts';

const resp = (captionTracks: unknown) => ({ captions: { playerCaptionsTracklistRenderer: { captionTracks } } });

describe('YouTube 字幕判斷', () => {
  // U14
  it('沒有字幕軌時不算有字幕', () => {
    expect(hasCaptions(resp([]))).toBe(false);
    expect(hasCaptions({})).toBe(false);
    expect(hasCaptions({ captions: {} })).toBe(false);
    expect(hasCaptions(null)).toBe(false);
    expect(hasCaptions(undefined)).toBe(false);
    expect(hasCaptions('壞資料')).toBe(false);
  });

  it('自動字幕與手動字幕都算有字幕', () => {
    expect(hasCaptions(resp([{ kind: 'asr', languageCode: 'en' }]))).toBe(true); // 只認手動字幕會讓自動字幕影片被誤判成沒字幕
    expect(hasCaptions(resp([{ languageCode: 'ja' }]))).toBe(true);
  });
});

describe('YouTube 聽譯動作', () => {
  const on: SiteRule = { auto: false, listen: true };
  const base = { rule: on as SiteRule | undefined, hasCaptions: false as boolean | null, playing: true, onWatchPage: true, listening: false };

  // U15
  it('規則開、沒字幕、播放中、觀看頁、未聽才開始', () => {
    expect(listenAction(base)).toBe('start');
  });

  it('不符合條件時不開始', () => {
    expect(listenAction({ ...base, hasCaptions: null })).toBe('none'); // 還不知道有沒有字幕,不能先聽
    expect(listenAction({ ...base, playing: false })).toBe('none');
    expect(listenAction({ ...base, hasCaptions: true })).toBe('none');
    expect(listenAction({ ...base, onWatchPage: false })).toBe('none');
    expect(listenAction({ ...base, rule: undefined })).toBe('none');
    expect(listenAction({ ...base, rule: { auto: true } })).toBe('none'); // 沒開 listen
  });

  it('聽的途中條件消失就停止', () => {
    const listening = { ...base, listening: true };
    expect(listenAction({ ...listening, hasCaptions: true })).toBe('stop'); // 換到有字幕的影片
    expect(listenAction({ ...listening, onWatchPage: false })).toBe('stop');
    expect(listenAction({ ...listening, rule: { auto: false, listen: false } })).toBe('stop');
    expect(listenAction({ ...listening, rule: undefined })).toBe('stop');
  });

  it('聽的時候暫停或條件不變時維持原狀', () => {
    expect(listenAction({ ...base, listening: true, playing: false })).toBe('none'); // 暫停就停會讓每次暫停都重開一輪
    expect(listenAction({ ...base, listening: true })).toBe('none');
  });

  it('未在聽且不想聽時什麼都不做', () => {
    expect(listenAction({ ...base, rule: undefined, hasCaptions: null, playing: false })).toBe('none');
  });
});
