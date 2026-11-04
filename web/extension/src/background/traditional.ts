// 本地小模型(Hy-MT2 1.8B)偶爾不照提示輸出簡體:目標是台灣繁體時,簡體譯文轉成台灣繁體與用語。
// 只轉簡體:cn→twp 套在已是台灣繁體的句子上會改錯字與換詞(干擾→幹擾、項目→專案)。與聽譯字幕的後處理一致。
import * as OpenCC from 'opencc-js';

let toTw: ((s: string) => string) | null = null;
let toCn: ((s: string) => string) | null = null;

/** 這段是不是簡體:轉成簡體不變、轉成繁體會變 */
function isSimplified(text: string): boolean {
  toTw ??= OpenCC.Converter({ from: 'cn', to: 'twp' });
  toCn ??= OpenCC.Converter({ from: 'tw', to: 'cn' });
  return toCn(text) === text && toTw(text) !== text;
}

export function toTraditional(text: string, targetLang: string): string {
  if (targetLang !== 'zh-TW' || !isSimplified(text)) return text;
  return toTw!(text);
}
