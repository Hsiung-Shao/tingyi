import { parseTranslations } from './parse.ts';
import type { BatchCaller, Segment } from './types.ts';

/** 呼叫一次;缺的 id 只重送缺的那些段,最多重試 retries 次。回傳拿得到的譯文 */
export async function translateWithRetry(
  segments: Segment[],
  call: BatchCaller,
  retries = 1,
): Promise<Map<number, string>> {
  const result = new Map<number, string>();
  let todo = segments;
  for (let attempt = 0; attempt <= retries && todo.length > 0; attempt++) {
    const raw = await call(todo);
    const { translations, missing } = parseTranslations(
      raw,
      todo.map((s) => s.id),
    );
    for (const [id, text] of translations) result.set(id, text);
    const left = new Set(missing);
    todo = todo.filter((s) => left.has(s.id));
  }
  return result;
}
