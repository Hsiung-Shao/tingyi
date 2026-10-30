import type { BatchFormat, BatchLimits } from '@atx/core';

export type Route = 'bridge' | 'direct';
/** dual:原文下方加譯文;only:隱藏原文只顯示譯文 */
export type DisplayMode = 'dual' | 'only';
/** main:跳過導覽列、頁首頁尾等介面區;page:整頁都翻 */
export type Scope = 'main' | 'page';

export interface Settings {
  route: Route;
  targetLang: string;
  displayMode: DisplayMode;
  scope: Scope;
  bridge: { url: string; token: string; provider: string; model: string };
  direct: {
    baseUrl: string;
    model: string;
    apiKey: string;
    jsonMode: boolean;
    format: BatchFormat;
    /** JSON 字串,合併進 request body */
    extraBody: string;
  };
  limits: { bridge: BatchLimits; direct: BatchLimits };
  concurrency: number;
}

export const DEFAULT_SETTINGS: Settings = {
  route: 'bridge',
  targetLang: 'zh-TW',
  displayMode: 'dual',
  scope: 'main',
  bridge: { url: 'http://127.0.0.1:47821/mcp', token: '', provider: 'codex-cli', model: '' },
  direct: {
    baseUrl: 'http://127.0.0.1:11434/v1',
    model: '',
    apiKey: '',
    jsonMode: true,
    format: 'json',
    extraBody: '{}',
  },
  limits: { bridge: { maxSegments: 20, maxChars: 6000 }, direct: { maxSegments: 8, maxChars: 2500 } },
  concurrency: 2,
};

export const SETTINGS_KEY = 'atx:settings';

export async function loadSettings(): Promise<Settings> {
  const raw = (await chrome.storage.local.get(SETTINGS_KEY))[SETTINGS_KEY] as Partial<Settings> | undefined;
  return {
    ...DEFAULT_SETTINGS,
    ...raw,
    bridge: { ...DEFAULT_SETTINGS.bridge, ...raw?.bridge },
    direct: { ...DEFAULT_SETTINGS.direct, ...raw?.direct },
    limits: { ...DEFAULT_SETTINGS.limits, ...raw?.limits },
  };
}

export async function saveSettings(s: Settings): Promise<void> {
  await chrome.storage.local.set({ [SETTINGS_KEY]: s });
}

/** 實際送翻時用的批次上限;single 格式一次只送一段 */
export function effectiveLimits(s: Settings): BatchLimits {
  if (s.route === 'direct' && s.direct.format === 'single') return { maxSegments: 1, maxChars: 4000 };
  return s.limits[s.route];
}
