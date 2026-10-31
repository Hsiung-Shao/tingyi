import type { BatchLimits, PageContext, Segment } from '@atx/core';

export type ContentRequest =
  | { type: 'translate'; segments: Segment[]; context: PageContext }
  | { type: 'getLimits' }
  | { type: 'openOptions' };

export type TranslateResponse =
  /** error:有部分段落失敗(其餘段落仍有譯文) */
  | { ok: true; translations: { id: number; text: string }[]; error?: string }
  | { ok: false; error: string };

export type LimitsResponse = BatchLimits & { targetLang: string; concurrency: number };

/** appLink:向本機程式要端點並更新設定,回 LinkStatus */
export type PopupRequest = { type: 'listProviders' } | { type: 'appLink' };

export type ProvidersResponse =
  | { ok: true; providers: { id: string; label: string; available: boolean; version?: string; note?: string }[] }
  | { ok: false; error: string };

/** background / popup → content */
export type TabRequest = { type: 'toggle' } | { type: 'getState' };

export interface TabState {
  enabled: boolean;
  done: number;
  pending: number;
  errors: number;
  lastError?: string;
}
