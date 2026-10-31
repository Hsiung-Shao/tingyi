import type { BatchLimits, Segment } from './types.ts';

/** 依段數與字數上限切批;單段超過字數上限時自成一批 */
export function splitBatches(segments: Segment[], limits: BatchLimits): Segment[][] {
  const batches: Segment[][] = [];
  let cur: Segment[] = [];
  let chars = 0;
  for (const seg of segments) {
    const len = seg.text.length;
    if (cur.length > 0 && (cur.length >= limits.maxSegments || chars + len > limits.maxChars)) {
      batches.push(cur);
      cur = [];
      chars = 0;
    }
    cur.push(seg);
    chars += len;
  }
  if (cur.length > 0) batches.push(cur);
  return batches;
}
