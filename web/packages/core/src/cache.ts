import type { CacheScope, KeyValueStore, Segment } from './types.ts';

/** 快取鍵:SHA-256(provider|model|targetLang|text) 的 hex,前綴 `atx:` */
export async function cacheKey(scope: CacheScope, text: string): Promise<string> {
  const data = new TextEncoder().encode([scope.provider, scope.model, scope.targetLang, text].join('\u0000'));
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', data));
  return `atx:${Array.from(digest, (b) => b.toString(16).padStart(2, '0')).join('')}`;
}

/** 未命中的段翻譯失敗;partial 是已從快取拿到的譯文 */
export class PartialTranslationError extends Error {
  readonly partial: Map<number, string>;
  constructor(partial: Map<number, string>, cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
    this.name = 'PartialTranslationError';
    this.partial = partial;
  }
}

/** 包一層快取:命中的段不送後端,未命中的交給 translate,結果寫回 store */
export function withCache(
  store: KeyValueStore,
  scope: CacheScope,
  translate: (segments: Segment[]) => Promise<Map<number, string>>,
): (segments: Segment[]) => Promise<Map<number, string>> {
  return async (segments) => {
    const keys = await Promise.all(segments.map((s) => cacheKey(scope, s.text)));
    const hits = await store.get([...new Set(keys)]);
    const result = new Map<number, string>();
    const misses: Segment[] = [];
    const missKeys = new Map<number, string>();
    segments.forEach((s, i) => {
      const key = keys[i]!;
      const hit = hits[key];
      if (hit !== undefined) result.set(s.id, hit);
      else {
        misses.push(s);
        missKeys.set(s.id, key);
      }
    });
    if (misses.length === 0) return result;
    let fresh: Map<number, string>;
    try {
      fresh = await translate(misses);
    } catch (e) {
      // 已命中的快取照樣交回去,只有未命中的段算失敗
      throw new PartialTranslationError(result, e);
    }
    const entries: Record<string, string> = {};
    for (const [id, text] of fresh) {
      result.set(id, text);
      const key = missKeys.get(id);
      if (key) entries[key] = text;
    }
    if (Object.keys(entries).length > 0) await store.set(entries);
    return result;
  };
}
