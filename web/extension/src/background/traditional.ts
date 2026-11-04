// 本地小模型(Hy-MT2 1.8B)偶爾不照提示輸出簡體:目標是台灣繁體時,依設定把簡體譯文轉成繁體。
// 只轉簡體:整句是簡體就整句轉;否則依標點切小句,只轉簡體的小句(繁簡同形字不算)。與聽譯字幕的後處理一致。
import * as OpenCC from 'opencc-js';
import type { ZhConvert } from '../shared/settings.ts';

const converters = new Map<string, (s: string) => string>();
function converter(from: 'cn' | 'tw', to: 'cn' | 'tw' | 'twp'): (s: string) => string {
  const key = `${from}>${to}`;
  let c = converters.get(key);
  if (!c) {
    c = OpenCC.Converter({ from, to });
    converters.set(key, c);
  }
  return c;
}

/** 繁簡同形、兩邊都常用的字:判斷時不算(否則「我在台北」會被當成簡體轉成臺北) */
const SHARED_HAN = /[台里干后面只系范松斗板准云余制]/g;

/** 這段是不是簡體:去掉繁簡同形字後,轉成簡體不變、轉成繁體會變 */
function isSimplified(text: string): boolean {
  const core = text.replace(SHARED_HAN, '');
  return converter('tw', 'cn')(core) === core && converter('cn', 'tw')(core) !== core;
}

/** 依標點切成小句(標點留在小句尾) */
const CLAUSE = /[^。！？；，!?;,\n]+[。！？；，!?;,\n]*|[。！？；，!?;,\n]+/g;

export function toTraditional(text: string, targetLang: string, mode: ZhConvert = 'terms'): string {
  if (targetLang !== 'zh-TW' || mode === 'none') return text;
  const conv = converter('cn', mode === 'terms' ? 'twp' : 'tw');
  if (isSimplified(text)) return conv(text);
  return (text.match(CLAUSE) ?? [text]).map((c) => (isSimplified(c) ? conv(c) : c)).join('');
}
