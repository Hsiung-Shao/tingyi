import type { BatchFormat, PageContext, Segment } from '@atx/core';

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
  signal?: AbortSignal;
}

export interface Provider {
  id: string;
  label: string;
  /** 送給後端的批次格式,決定輸出怎麼解析;預設 json */
  format?: BatchFormat;
  /** CLI agent 走大批次;OpenAI 相容端點依設定 */
  limits: { maxSegments: number; maxChars: number };
  detect(): Promise<ProviderStatus>;
  /** 回傳後端原始輸出(交給 core 的 parseTranslations) */
  translate(req: TranslateRequest): Promise<string>;
}
