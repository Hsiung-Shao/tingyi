import { describe, expect, it } from 'vitest';
import { buildPrompt } from '../src/prompt.ts';

const segs = [{ id: 1, text: 'Hello world' }];

describe('buildPrompt single 格式', () => {
  it('目標 zh-TW 時用中文範本並指定繁體台灣用語', () => {
    const p = buildPrompt(segs, 'zh-TW', 'single');
    // Hy-MT2 等翻譯模型吃英文指示會輸出簡體,要用官方中文範本
    expect(p.user).toContain('翻译为繁体中文(台湾用语)');
    expect(p.user.endsWith('Hello world')).toBe(true);
  });

  it('目標非中文時用英文範本', () => {
    const p = buildPrompt(segs, 'ja', 'single');
    expect(p.user).toContain('Japanese');
    expect(p.user.endsWith('Hello world')).toBe(true);
  });
});
