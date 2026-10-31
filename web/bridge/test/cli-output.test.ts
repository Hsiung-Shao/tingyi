import { describe, expect, it } from 'vitest';
import { cliResultToText } from '../src/providers/cliProvider.ts';

describe('cliResultToText', () => {
  it('codex 沒有輸出檔時丟出含 stderr 的錯誤而不是把 stdout 當譯文', () => {
    expect(() =>
      cliResultToText('codex', { stdout: 'codex banner text', stderr: 'error: not logged in', exitCode: 1 }, undefined),
    ).toThrow(/not logged in/);
  });

  it('codex 有輸出檔時回傳檔案內容', () => {
    const text = cliResultToText('codex', { stdout: 'progress', stderr: '', exitCode: 0 }, '{"translations":[]}');
    expect(text).toBe('{"translations":[]}');
  });

  it('claude 回報錯誤時丟出錯誤', () => {
    const stdout = JSON.stringify({ is_error: true, result: 'Failed to authenticate' });
    expect(() => cliResultToText('claude', { stdout, stderr: '', exitCode: 1 })).toThrow(/authenticate/);
  });
});
