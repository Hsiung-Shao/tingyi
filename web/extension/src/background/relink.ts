import { PartialTranslationError } from '@atx/core';
import type { Route } from '../shared/settings.ts';

/**
 * 翻譯失敗時要不要向本機程式重新取得端點(聽譯重啟後 port、金鑰都換了)。
 */

/** 網路層失敗或金鑰不符(端點換了) */
export function isConnectionError(e: unknown): boolean {
  const msg = e instanceof Error ? e.message : String(e);
  return e instanceof TypeError || /Failed to fetch|ECONNREFUSED|端點回應 401|fetch failed/i.test(msg);
}

/**
 * 是否該重新連線再試一次。快取層會把失敗包成 PartialTranslationError(原錯誤放在 cause),
 * 包起來的連線錯誤也要重新連線,不能當成「部分失敗」直接回給頁面。
 * 只有直連路線要:聽譯只提供本地模型端點,bridge 連不上時重新取得端點也修不好,還會白白喚起聽譯。
 */
export function needsRelink(e: unknown, route: Route = 'direct'): boolean {
  if (route !== 'direct') return false;
  return isConnectionError(e) || (e instanceof PartialTranslationError && isConnectionError(e.cause));
}
