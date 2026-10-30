import { describe, expect, it } from 'vitest';
import { cacheKey, withCache } from '../src/cache.ts';
import type { CacheScope, KeyValueStore, Segment } from '../src/types.ts';

function memoryStore(): KeyValueStore {
  const data: Record<string, string> = {};
  return {
    async get(keys) {
      const out: Record<string, string> = {};
      for (const k of keys) {
        const v = data[k];
        if (v !== undefined) out[k] = v;
      }
      return out;
    },
    async set(entries) {
      Object.assign(data, entries);
    },
  };
}

const scope: CacheScope = { provider: 'claude', model: 'sonnet', targetLang: 'zh-TW' };

describe('withCache', () => {
  it('T10 相同原文連送兩次後端只被呼叫一次', async () => {
    const store = memoryStore();
    let backendCalls = 0;
    const translate = async (segs: Segment[]) => {
      backendCalls++;
      return new Map(segs.map((s) => [s.id, `譯:${s.text}`] as [number, string]));
    };
    const cached = withCache(store, scope, translate);
    const first = await cached([{ id: 1, text: 'Hello' }]);
    const second = await cached([{ id: 7, text: 'Hello' }]);
    expect(backendCalls).toBe(1);
    expect(first.get(1)).toBe('譯:Hello');
    expect(second.get(7)).toBe('譯:Hello');
  });

  it('T10 只把未命中的段送給後端', async () => {
    const store = memoryStore();
    const sent: string[][] = [];
    const translate = async (segs: Segment[]) => {
      sent.push(segs.map((s) => s.text));
      return new Map(segs.map((s) => [s.id, `譯:${s.text}`] as [number, string]));
    };
    const cached = withCache(store, scope, translate);
    await cached([{ id: 1, text: 'A' }]);
    const r = await cached([
      { id: 1, text: 'A' },
      { id: 2, text: 'B' },
    ]);
    expect(sent).toEqual([['A'], ['B']]);
    expect(r.get(1)).toBe('譯:A');
    expect(r.get(2)).toBe('譯:B');
  });

  it('T10 後端、模型或語言不同時不共用快取', async () => {
    const store = memoryStore();
    let backendCalls = 0;
    const translate = async (segs: Segment[]) => {
      backendCalls++;
      return new Map(segs.map((s) => [s.id, 'x'] as [number, string]));
    };
    await withCache(store, scope, translate)([{ id: 1, text: 'Hello' }]);
    await withCache(store, { ...scope, model: 'opus' }, translate)([{ id: 1, text: 'Hello' }]);
    await withCache(store, { ...scope, targetLang: 'ja' }, translate)([{ id: 1, text: 'Hello' }]);
    await withCache(store, { ...scope, provider: 'codex' }, translate)([{ id: 1, text: 'Hello' }]);
    expect(backendCalls).toBe(4);
    await withCache(store, scope, translate)([{ id: 1, text: 'Hello' }]);
    expect(backendCalls).toBe(4);
  });
});

describe('cacheKey', () => {
  it('T10 鍵為 atx: 加 64 位 hex 且 scope 或原文不同則不同', async () => {
    const k = await cacheKey(scope, 'Hello');
    expect(k).toMatch(/^atx:[0-9a-f]{64}$/);
    expect(await cacheKey(scope, 'Hello')).toBe(k);
    expect(await cacheKey(scope, 'World')).not.toBe(k);
    expect(await cacheKey({ ...scope, model: 'opus' }, 'Hello')).not.toBe(k);
  });
});
