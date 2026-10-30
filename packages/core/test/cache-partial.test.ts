import { describe, expect, it } from 'vitest';
import { PartialTranslationError, withCache } from '../src/cache.ts';
import type { KeyValueStore, Segment } from '../src/types.ts';

function memoryStore(): KeyValueStore {
  const data: Record<string, string> = {};
  return {
    async get(keys) {
      return Object.fromEntries(keys.filter((k) => k in data).map((k) => [k, data[k]!]));
    },
    async set(entries) {
      Object.assign(data, entries);
    },
  };
}

const scope = { provider: 'p', model: 'm', targetLang: 'zh-TW' };

describe('withCache 部分失敗', () => {
  it('未命中的部分失敗時錯誤帶著已命中的快取譯文', async () => {
    const store = memoryStore();
    const ok = async (segs: Segment[]) => new Map(segs.map((s) => [s.id, `譯${s.text}`] as [number, string]));
    await withCache(store, scope, ok)([{ id: 1, text: 'A' }]);
    const failing = withCache(store, scope, async () => {
      throw new Error('CLI 逾時');
    });
    const err = await failing([
      { id: 1, text: 'A' },
      { id: 2, text: 'B' },
    ]).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(PartialTranslationError);
    expect((err as PartialTranslationError).partial.get(1)).toBe('譯A');
    expect((err as PartialTranslationError).message).toContain('CLI 逾時');
  });
});
