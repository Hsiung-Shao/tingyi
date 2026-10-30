import type { PageContext, Segment } from '@atx/core';

export interface ProviderStatus {
  available: boolean;
  version?: string;
  note?: string;
}

export interface TranslateRequest {
  segments: Segment[];
  targetLang: string;
  model?: string;
  context?: PageContext;
}

export interface Provider {
  id: string;
  label: string;
  /** CLI agent 走大批次;OpenAI 相容端點依設定 */
  limits: { maxSegments: number; maxChars: number };
  detect(): Promise<ProviderStatus>;
  /** 回傳後端原始輸出(交給 core 的 parseTranslations) */
  translate(req: TranslateRequest): Promise<string>;
}
