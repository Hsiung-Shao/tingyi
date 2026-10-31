import { PartialTranslationError } from '@atx/core';
import { describe, expect, it } from 'vitest';
import { needsRelink } from '../src/background/relink.ts';

describe('翻譯失敗時是否重新連線本機程式', () => {
  // T27
  it('連線錯誤被快取層包起來時仍要重新連線', () => {
    expect(needsRelink(new TypeError('Failed to fetch'))).toBe(true);
    expect(needsRelink(new PartialTranslationError(new Map(), new TypeError('Failed to fetch')))).toBe(true);
    // partial 有內容也一樣
    expect(needsRelink(new PartialTranslationError(new Map([[0, '你好']]), new TypeError('Failed to fetch')))).toBe(true);
    // 金鑰不符(端點換了)
    expect(needsRelink(new PartialTranslationError(new Map(), new Error('OpenAI 相容端點回應 401:bad key')))).toBe(true);
  });

  // T27b
  it('伺服器忙碌或一般錯誤不重新連線', () => {
    expect(needsRelink(new PartialTranslationError(new Map([[0, '你好']]), new Error('OpenAI 相容端點回應 500:busy')))).toBe(false);
    expect(needsRelink(new Error('JSON 解析失敗'))).toBe(false);
  });

  // T27c
  it('bridge 路線的連線錯誤不重新連線,direct 路線才重新連線', () => {
    const raw = new TypeError('Failed to fetch');
    const wrapped = new PartialTranslationError(new Map(), new TypeError('Failed to fetch'));
    expect(needsRelink(raw, 'bridge')).toBe(false);
    expect(needsRelink(wrapped, 'bridge')).toBe(false);
    expect(needsRelink(raw, 'direct')).toBe(true);
    expect(needsRelink(wrapped, 'direct')).toBe(true);
  });
});
