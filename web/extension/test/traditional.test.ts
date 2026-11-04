import { describe, expect, it } from 'vitest';
import { toTraditional } from '../src/background/traditional.ts';

describe('譯文簡轉繁', () => {
  // Z1
  it('目標台灣繁體時簡體譯文轉成繁體與台灣用語', () => {
    // 軟件→軟體、网络→網路是台灣用語;只做字形轉換會留下「軟件」「網絡」
    expect(toTraditional('这是简体中文软件，请检查网络。', 'zh-TW')).toBe('這是簡體中文軟體，請檢查網路。');
  });

  // Z2
  it('已是繁體的譯文不被改壞', () => {
    expect(toTraditional('這是繁體中文的字幕。', 'zh-TW')).toBe('這是繁體中文的字幕。');
  });

  // Z3
  it('其他目標語言不轉', () => {
    expect(toTraditional('这是简体', 'zh-CN')).toBe('这是简体');
    expect(toTraditional('これはテストです', 'ja')).toBe('これはテストです');
  });

  // Z4
  it('已是台灣繁體的譯文不被改字或換詞', () => {
    const sentences = ['干擾訊號很強', '鄰里之間', '以下項目請參考文件', '我支持你的決定', '台灣的平台', '這次優化了速度'];
    // 逐字轉換會把「干擾」「鄰里」「支持」「優化」等繁體詞改壞;一次列出全部被改的句子
    const changed = sentences.map((t) => [t, toTraditional(t, 'zh-TW')]).filter(([a, b]) => a !== b);
    expect(changed).toEqual([]);
  });

  // Z6
  it('簡繁混雜時只轉簡體的小句', () => {
    // 整段一起判斷會讓繁體小句被改字,或簡體小句因整段含繁體而漏轉
    expect(toTraditional('今天天气很好。明天會下雨。', 'zh-TW')).toBe('今天天氣很好。明天會下雨。');
    expect(toTraditional('这是软件，裡面有東西', 'zh-TW')).toBe('這是軟體，裡面有東西');
  });
});
