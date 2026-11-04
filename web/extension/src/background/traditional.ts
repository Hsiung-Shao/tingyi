// 本地小模型(Hy-MT2 1.8B)偶爾不照提示輸出簡體:目標是台灣繁體時,譯文一律轉成台灣繁體與用語。
// 與聽譯字幕的後處理一致(Rust 端 OpenCC s2twp)。
import * as OpenCC from 'opencc-js/cn2t';

let converter: ((s: string) => string) | null = null;

export function toTraditional(text: string, targetLang: string): string {
  if (targetLang !== 'zh-TW') return text;
  converter ??= OpenCC.Converter({ from: 'cn', to: 'twp' });
  return converter(text);
}
