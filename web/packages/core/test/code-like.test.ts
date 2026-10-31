import { describe, expect, it } from 'vitest';
import { isCodeLike } from '../src/lang.ts';

describe('isCodeLike 中日文', () => {
  it('含英文詞的中日文句子不算程式碼', () => {
    for (const t of ['JavaScriptの基本を学ぶ', 'Node.js开发入门教程', 'iPhone新機能の紹介', 'GitHub使用教學']) {
      expect(isCodeLike(t), t).toBe(false);
    }
  });

  it('純 ASCII 的識別字仍算程式碼', () => {
    expect(isCodeLike('GitHubActions')).toBe(true);
    expect(isCodeLike('Node.js')).toBe(true);
  });
});
