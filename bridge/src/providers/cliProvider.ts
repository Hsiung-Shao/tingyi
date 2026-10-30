import { mkdtemp, readFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { buildPrompt } from '@atx/core';
import { runProcess } from '../process.ts';
import { resolveCommand, sanitizedEnv } from '../resolve.ts';
import { buildCliInvocation, type CliKind } from './cli.ts';
import type { Provider, ProviderStatus, TranslateRequest } from './types.ts';

const LABELS: Record<CliKind, string> = {
  claude: 'Claude Code CLI(訂閱)',
  codex: 'Codex CLI(ChatGPT 訂閱)',
  gemini: 'Gemini CLI',
};

export interface CliProviderOptions {
  kind: CliKind;
  timeoutMs?: number;
  /** 提示注入閘門未驗證的後端標為高風險,仍可由設定檔開啟 */
  note?: string;
}

/** 從各 CLI 的輸出取出模型回覆文字 */
export function extractCliOutput(kind: CliKind, stdout: string, outputFileText?: string): string {
  if (kind === 'codex') return outputFileText ?? stdout;
  const start = stdout.indexOf('{');
  if (start < 0) return stdout;
  try {
    const data = JSON.parse(stdout.slice(start)) as Record<string, unknown>;
    if (kind === 'claude') {
      if (data.is_error) throw new Error(`claude: ${String(data.result)}`);
      if (data.structured_output) return JSON.stringify(data.structured_output);
      return String(data.result ?? '');
    }
    const err = data.error as { message?: string } | undefined;
    if (err?.message) throw new Error(`gemini: ${err.message}`);
    return String(data.response ?? '');
  } catch (e) {
    if (e instanceof SyntaxError) return stdout;
    throw e;
  }
}

export function createCliProvider(opts: CliProviderOptions): Provider {
  const { kind } = opts;
  const timeoutMs = opts.timeoutMs ?? 180_000;
  return {
    id: `${kind}-cli`,
    label: LABELS[kind],
    limits: { maxSegments: 20, maxChars: 6000 },
    async detect(): Promise<ProviderStatus> {
      const resolved = resolveCommand(kind);
      if (!resolved) return { available: false, note: `找不到 ${kind} 指令` };
      try {
        const r = await runProcess({
          command: resolved.command,
          args: [...resolved.prefixArgs, '--version'],
          stdin: '',
          cwd: os.tmpdir(),
          env: sanitizedEnv(),
          timeoutMs: 20_000,
        });
        return { available: r.exitCode === 0, version: r.stdout.trim().split('\n')[0], note: opts.note };
      } catch (e) {
        return { available: false, note: String(e) };
      }
    },
    async translate(req: TranslateRequest): Promise<string> {
      const resolved = resolveCommand(kind);
      if (!resolved) throw new Error(`找不到 ${kind} 指令`);
      const prompt = buildPrompt(req.segments, req.targetLang, 'json', req.context);
      const workDir = await mkdtemp(path.join(os.tmpdir(), 'atx-'));
      try {
        const inv = buildCliInvocation(kind, {
          systemPrompt: prompt.system,
          payload: prompt.user,
          model: req.model,
          workDir,
        });
        const r = await runProcess({
          command: resolved.command,
          args: [...resolved.prefixArgs, ...inv.args],
          stdin: inv.stdin,
          cwd: workDir,
          env: sanitizedEnv(),
          timeoutMs,
        });
        const fileText = inv.outputFile ? await readFile(inv.outputFile, 'utf8').catch(() => undefined) : undefined;
        const out = extractCliOutput(kind, r.stdout, fileText);
        if (r.exitCode !== 0 && !out.trim()) {
          throw new Error(`${kind} 結束碼 ${r.exitCode}:${r.stderr.slice(-300)}`);
        }
        return out;
      } finally {
        await rm(workDir, { recursive: true, force: true }).catch(() => {});
      }
    },
  };
}
