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
