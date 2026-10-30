export interface Segment {
  id: number;
  text: string;
}

export interface PageContext {
  title?: string;
  url?: string;
}

export interface BatchLimits {
  maxSegments: number;
  maxChars: number;
}

/** json:多段編號 JSON 一次送;single:一次一段純文字(給 Hy-MT2 這類只會翻譯的模型) */
export type BatchFormat = 'json' | 'single';

export interface PromptPair {
  system: string;
  user: string;
}

export interface ParseResult {
  translations: Map<number, string>;
  missing: number[];
}

/** 送一批段落給後端,回傳後端原始輸出字串 */
export type BatchCaller = (segments: Segment[]) => Promise<string>;

export interface KeyValueStore {
  get(keys: string[]): Promise<Record<string, string>>;
  set(entries: Record<string, string>): Promise<void>;
}

export interface CacheScope {
  provider: string;
  model: string;
  targetLang: string;
}
