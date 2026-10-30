import type { BatchFormat, BatchLimits } from '@atx/core';

export type Route = 'bridge' | 'direct';

export interface Settings {
  route: Route;
  targetLang: string;
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

const KEY = 'atx:settings';

export async function loadSettings(): Promise<Settings> {
  const raw = (await chrome.storage.local.get(KEY))[KEY] as Partial<Settings> | undefined;
  return {
    ...DEFAULT_SETTINGS,
    ...raw,
    bridge: { ...DEFAULT_SETTINGS.bridge, ...raw?.bridge },
    direct: { ...DEFAULT_SETTINGS.direct, ...raw?.direct },
    limits: { ...DEFAULT_SETTINGS.limits, ...raw?.limits },
  };
}

export async function saveSettings(s: Settings): Promise<void> {
  await chrome.storage.local.set({ [KEY]: s });
}

/** 實際送翻時用的批次上限;single 格式一次只送一段 */
export function effectiveLimits(s: Settings): BatchLimits {
  if (s.route === 'direct' && s.direct.format === 'single') return { maxSegments: 1, maxChars: 4000 };
  return s.limits[s.route];
}
