import type { BatchLimits, PageContext, Segment } from '@atx/core';

export type ContentRequest =
  | { type: 'translate'; segments: Segment[]; context: PageContext }
  | { type: 'getLimits' };

export type TranslateResponse =
  | { ok: true; translations: { id: number; text: string }[] }
  | { ok: false; error: string };

export type LimitsResponse = BatchLimits & { targetLang: string };

export type PopupRequest = { type: 'listProviders' };

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
