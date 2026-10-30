import { describe, expect, it } from 'vitest';
import { splitBatches } from '../src/batch.ts';
import type { Segment } from '../src/types.ts';

const seg = (id: number, len = 10): Segment => ({ id, text: 'x'.repeat(len) });

describe('splitBatches', () => {
  it('T6 30 段依 20 段上限切成 20 與 10 兩批', () => {
    const segments = Array.from({ length: 30 }, (_, i) => seg(i + 1));
    const batches = splitBatches(segments, { maxSegments: 20, maxChars: 6000 });
    expect(batches.map((b) => b.length)).toEqual([20, 10]);
  });

  it('T6 切批後順序不變且每段只出現一次', () => {
    const segments = Array.from({ length: 30 }, (_, i) => seg(i + 1));
    const batches = splitBatches(segments, { maxSegments: 20, maxChars: 6000 });
    expect(batches.flat().map((s) => s.id)).toEqual(segments.map((s) => s.id));
  });

  it('T6 每批總字數都不超過字數上限', () => {
    const segments = Array.from({ length: 12 }, (_, i) => seg(i + 1, 1000));
    const batches = splitBatches(segments, { maxSegments: 20, maxChars: 6000 });
    expect(batches.length).toBeGreaterThan(1);
    for (const b of batches) {
      expect(b.reduce((n, s) => n + s.text.length, 0)).toBeLessThanOrEqual(6000);
    }
    expect(batches.flat()).toHaveLength(12);
  });

  it('T6 單段超過字數上限時自成一批', () => {
    const big = seg(2, 7000);
    const segments = [seg(1, 100), big, seg(3, 100)];
    const batches = splitBatches(segments, { maxSegments: 20, maxChars: 6000 });
    const bigBatch = batches.find((b) => b.some((s) => s.id === 2));
    expect(bigBatch).toEqual([big]);
    expect(batches.flat().map((s) => s.id)).toEqual([1, 2, 3]);
  });

  it('T6 空輸入回傳空陣列', () => {
    expect(splitBatches([], { maxSegments: 20, maxChars: 6000 })).toEqual([]);
  });
});
