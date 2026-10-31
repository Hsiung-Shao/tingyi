import type { BatchFormat, BatchLimits } from '@atx/core';

export type Route = 'bridge' | 'direct';
/** dual:原文下方加譯文;only:隱藏原文只顯示譯文 */
export type DisplayMode = 'dual' | 'only';
/** main:跳過導覽列、頁首頁尾等介面區;page:整頁都翻 */
export type Scope = 'main' | 'page';

/** auto:端點由本機程式自動提供;manual:使用者自己填 */
export type Connection = 'auto' | 'manual';

export interface Settings {
  route: Route;
  connection: Connection;
  /** 是否曾經自動連上本機程式(第一次連上時才把 route 設成本地模型) */
  linked: boolean;
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
    /** 本機程式指派給網頁翻譯的 llama-server slot(輪流帶 id_slot);空 = 不帶 */
    slots: number[];
  };
  limits: { bridge: BatchLimits; direct: BatchLimits };
  concurrency: number;
}

export const DEFAULT_SETTINGS: Settings = {
  route: 'bridge',
  connection: 'auto',
  linked: false,
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
    slots: [],
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
