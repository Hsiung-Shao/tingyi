import type { ParseResult } from './types.ts';

const PREFIX =
  /^(?:以下是翻譯|以下為翻譯|以下是譯文|翻譯如下|譯文如下|以下是翻译|以下是译文|翻译如下|译文如下|譯文|翻譯|译文|翻译|translation|translated text|here is the translation)\s*[:：]\s*/i;
const FENCE = /^```[\w-]*\s*\n([\s\S]*?)\n?```$/;

/** 清掉模型常加的前綴(「以下是翻譯:」「Translation:」等)與包住全文的引號/程式碼框 */
export function cleanText(text: string): string {
  let t = text.trim();
  const fence = FENCE.exec(t);
  if (fence) t = (fence[1] ?? '').trim();
  t = t.replace(PREFIX, '').trim();
  if (/^"[^"]*"$/.test(t) || /^“[^“”]*”$/.test(t)) t = t.slice(1, -1).trim();
  return t;
}

function extractJson(raw: string): unknown {
  const fence = /```(?:json)?\s*\n([\s\S]*?)\n?```/.exec(raw);
  const candidates = [fence?.[1], raw];
  for (const c of candidates) {
    if (!c) continue;
    const start = c.indexOf('{');
    const end = c.lastIndexOf('}');
    if (start < 0 || end <= start) continue;
    try {
      return JSON.parse(c.slice(start, end + 1));
    } catch {
      // 繼續試下一個候選
    }
  }
  return undefined;
}

/**
 * 解析後端輸出。先試 JSON `{"translations":[{id,text}]}`(可被 ```json 包住或前後有雜訊),
 * 失敗再用 `%%` 分隔、段數等於 expectedIds 時依位置對應。只收 expectedIds 內的 id。
 */
export function parseTranslations(raw: string, expectedIds: number[]): ParseResult {
  const expected = new Set(expectedIds);
  const translations = new Map<number, string>();

  const data = extractJson(raw) as { translations?: unknown } | undefined;
  if (data && Array.isArray(data.translations)) {
    for (const item of data.translations as { id?: unknown; text?: unknown }[]) {
      const id = Number(item?.id);
      if (expected.has(id) && typeof item.text === 'string' && !translations.has(id)) {
        translations.set(id, cleanText(item.text));
      }
    }
  } else {
    const parts = raw.split(/\n?[ \t]*%%[ \t]*\n?/).map(cleanText);
    if (parts.length === expectedIds.length && parts.every((p) => p.length > 0)) {
      expectedIds.forEach((id, i) => translations.set(id, parts[i]!));
    }
  }
  return { translations, missing: expectedIds.filter((id) => !translations.has(id)) };
}
