import { describe, expect, it } from 'vitest';
import { cleanText, parseTranslations } from '../src/parse.ts';

describe('parseTranslations', () => {
  it('T7 亂序的 JSON 輸出依 id 對回', () => {
    const raw = JSON.stringify({
      translations: [
        { id: 3, text: '第三' },
        { id: 1, text: '第一' },
        { id: 2, text: '第二' },
      ],
    });
    const r = parseTranslations(raw, [1, 2, 3]);
    expect(r.translations.get(1)).toBe('第一');
    expect(r.translations.get(2)).toBe('第二');
    expect(r.translations.get(3)).toBe('第三');
    expect(r.missing).toEqual([]);
  });

  it('T7 多出的 id 被忽略', () => {
    const raw = JSON.stringify({
      translations: [
        { id: 1, text: '甲' },
        { id: 99, text: '多餘' },
      ],
    });
    const r = parseTranslations(raw, [1]);
    expect([...r.translations.keys()]).toEqual([1]);
    expect(r.translations.has(99)).toBe(false);
  });

  it('T7 缺少的 id 列在 missing', () => {
    const raw = JSON.stringify({ translations: [{ id: 2, text: '乙' }] });
    const r = parseTranslations(raw, [1, 2, 3]);
    expect([...r.translations.keys()]).toEqual([2]);
    expect(r.missing).toEqual([1, 3]);
  });

  it('T7 被 ```json 包住且前後有雜訊仍可解析', () => {
    const raw = '好的,結果如下:\n```json\n{"translations":[{"id":1,"text":"你好"}]}\n```\n以上。';
    const r = parseTranslations(raw, [1]);
    expect(r.translations.get(1)).toBe('你好');
    expect(r.missing).toEqual([]);
  });

  it('T8 前綴加 %% 分隔的輸出依位置對應並清掉前綴', () => {
    const raw = '以下是翻譯:\n第一段譯文\n%%\n第二段譯文\n%%\n第三段譯文';
    const r = parseTranslations(raw, [10, 20, 30]);
    expect(r.translations.size).toBe(3);
    expect(r.translations.get(10)).toBe('第一段譯文');
    expect(r.translations.get(20)).toBe('第二段譯文');
    expect(r.translations.get(30)).toBe('第三段譯文');
    expect(r.missing).toEqual([]);
  });

  it('T8 分隔段數與預期不符時不硬對應', () => {
    const r = parseTranslations('甲\n%%\n乙', [1, 2, 3]);
    expect(r.translations.size).toBe(0);
    expect(r.missing).toEqual([1, 2, 3]);
  });
});

describe('cleanText', () => {
  it('T8 清掉「以下是翻譯:」前綴', () => {
    expect(cleanText('以下是翻譯:你好世界')).toBe('你好世界');
  });

  it('T8 清掉 Translation: 前綴', () => {
    expect(cleanText('Translation: 你好世界')).toBe('你好世界');
  });

  it('T8 去掉包住全文的程式碼框', () => {
    expect(cleanText('```\n你好世界\n```')).toBe('你好世界');
  });

  it('T8 沒有前綴的文字維持原樣', () => {
    expect(cleanText('你好世界')).toBe('你好世界');
  });
});
