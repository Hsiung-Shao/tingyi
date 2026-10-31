import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { buildCliInvocation, type CliKind } from '../src/providers/cli.ts';

const PAYLOAD = 'UNIQUE-PAYLOAD-123';
let workDir: string;

beforeAll(() => {
  workDir = mkdtempSync(join(tmpdir(), 'atx-cli-test-'));
});
afterAll(() => {
  rmSync(workDir, { recursive: true, force: true });
});

const build = (kind: CliKind) =>
  buildCliInvocation(kind, { systemPrompt: 'SYSTEM-PROMPT-XYZ', payload: PAYLOAD, workDir });

/** 找旗標後面緊接的參數 */
const after = (args: string[], flag: string): string | undefined => {
  const i = args.indexOf(flag);
  return i === -1 ? undefined : args[i + 1];
};

describe('buildCliInvocation', () => {
  it('T14 claude 命令含關閉工具的旗標', () => {
    const { args } = build('claude');
    expect(args).toContain('-p');
    expect(args).toContain('--tools');
    expect(after(args, '--tools')).toBe('');
    expect(args).toContain('--strict-mcp-config');
  });

  it('T14 codex 命令含唯讀沙箱與關閉 shell 工具', () => {
    const { args } = build('codex');
    expect(args).toContain('exec');
    expect(after(args, '--sandbox')).toBe('read-only');
    expect(args).toContain('features.shell_tool=false');
  });

  it('T14 gemini 命令含 -e none', () => {
    const { args } = build('gemini');
    expect(after(args, '-e')).toBe('none');
  });

  it('gemini 只允許一個不存在的 MCP server 名稱,擋掉使用者全域 MCP', () => {
    const { args } = build('gemini');
    const name = after(args, '--allowed-mcp-server-names');
    expect(name).toBeDefined();
    expect(name!.startsWith('-')).toBe(false);
  });

  it('claude 的 system prompt 參數不含頁面標題這類不可信內容的空間(只放固定指示)', () => {
    const inv = buildCliInvocation('claude', { systemPrompt: 'FIXED', payload: PAYLOAD, workDir });
    expect(after(inv.args, '--system-prompt')).toBe('FIXED');
  });

  for (const kind of ['claude', 'codex', 'gemini'] as const) {
    it(`T14 ${kind} 的原文只走 stdin 不出現在任何參數`, () => {
      const inv = build(kind);
      expect(inv.args.length).toBeGreaterThan(0);
      expect(inv.args.some((a) => a.includes(PAYLOAD))).toBe(false);
      expect(inv.stdin).toContain(PAYLOAD);
    });
  }
});
