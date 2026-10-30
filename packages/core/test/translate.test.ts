import { describe, expect, it } from 'vitest';
import { translateWithRetry } from '../src/translate.ts';
import type { Segment } from '../src/types.ts';

const json = (items: Array<[number, string]>) =>
  JSON.stringify({ translations: items.map(([id, text]) => ({ id, text })) });

describe('translateWithRetry', () => {
  it('T9 輸出少 2 個 id 時第二次只重送缺的 2 段', async () => {
    const segments: Segment[] = [1, 2, 3, 4, 5].map((id) => ({ id, text: `原文${id}` }));
    const received: number[][] = [];
    const call = async (segs: Segment[]) => {
      received.push(segs.map((s) => s.id));
      if (received.length === 1) return json([[1, 'a'], [2, 'b'], [3, 'c']]);
      return json(segs.map((s) => [s.id, `重送${s.id}`] as [number, string]));
    };
    const result = await translateWithRetry(segments, call);
    expect(received).toEqual([[1, 2, 3, 4, 5], [4, 5]]);
    expect(result.get(1)).toBe('a');
    expect(result.get(4)).toBe('重送4');
    expect(result.get(5)).toBe('重送5');
    expect(result.size).toBe(5);
  });

  it('T9 一次就齊全時不重試', async () => {
    const segments: Segment[] = [1, 2].map((id) => ({ id, text: `t${id}` }));
    let calls = 0;
    const result = await translateWithRetry(segments, async () => {
      calls++;
      return json([[1, 'a'], [2, 'b']]);
    });
    expect(calls).toBe(1);
    expect(result.size).toBe(2);
  });

  it('T9 重試仍缺時只回傳拿得到的譯文且不超過重試次數', async () => {
    const segments: Segment[] = [1, 2, 3].map((id) => ({ id, text: `t${id}` }));
    let calls = 0;
    const result = await translateWithRetry(
      segments,
      async () => {
        calls++;
        return json([[1, 'a']]);
      },
      1,
    );
    expect(calls).toBe(2);
    expect([...result.keys()]).toEqual([1]);
  });
});
