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
});
