/** 文字看起來已經是目標語言(MVP 只判斷 zh-*:漢字佔比 >= 0.6)就回 true */
export function isLikelyTargetLang(text: string, targetLang: string): boolean {
  if (!targetLang.startsWith('zh')) return false;
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
