/** 只出現在簡體的常用字;目標繁中時含這些字的段落要翻(轉繁) */
export const SIMPLIFIED_ONLY = /[这们个为语说时机译历计习软词统应网络设从进运认让务实经过还发单页简体国际]/;

/**
 * 文字看起來已經是目標語言就回 true。只判斷 zh-*:漢字佔比 >= 0.6;
 * 目標 zh-TW 時含簡體專用字不算(要轉繁)。日文全漢字標題要靠呼叫端看 lang 屬性排除。
 */
export function isLikelyTargetLang(text: string, targetLang: string): boolean {
  if (!targetLang.startsWith('zh')) return false;
  if (targetLang === 'zh-TW' && SIMPLIFIED_ONLY.test(text)) return false;
  const han = text.match(/\p{Script=Han}/gu)?.length ?? 0;
  // 分母只算「文字」:漢字、拉丁字母、假名、韓文,不算標點與數字
  const letters = text.match(/[\p{Script=Han}\p{Script=Latin}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/gu)?.length ?? 0;
  return letters > 0 && han / letters >= 0.6;
}

/**
 * 看起來像程式識別字的單一詞(沒有空白):駝峰(aB)、含底線、結尾 ()、含 .x 或 ::。
 * 例:DeferredRequestInit、fetch()、snake_case、a.b.c 回 true;Install、Hello world 回 false。
 */
export function isCodeLike(text: string): boolean {
  const t = text.trim();
  // 中日文句子本來就沒有空白,只有整段都是 ASCII 可見字元的單詞才可能是識別字
  if (t.length < 2 || !/^[\x21-\x7e]+$/.test(t)) return false;
  return /[a-z][A-Z]/.test(t) || /\w_\w/.test(t) || /\(\)$/.test(t) || /\.\w/.test(t) || t.includes('::');
}

const LANG_NAMES: Record<string, string> = {
  'zh-TW': 'Traditional Chinese (Taiwan)',
  'zh-CN': 'Simplified Chinese',
  en: 'English',
  ja: 'Japanese',
  ko: 'Korean',
};

export function langName(code: string): string {
  return LANG_NAMES[code] ?? code;
}
