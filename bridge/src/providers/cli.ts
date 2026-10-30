import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { TRANSLATIONS_SCHEMA } from '@atx/core';

export type CliKind = 'claude' | 'codex' | 'gemini';

export interface CliBuildOptions {
  systemPrompt: string;
  /** 段落 JSON;只能經 stdin 傳,不得出現在 args */
  payload: string;
  model?: string;
  /** 每次呼叫的空暫存目錄(放 schema、輸出檔) */
  workDir: string;
}

export interface CliInvocation {
  command: string;
  args: string[];
  stdin: string;
  /** codex 的最後訊息檔;其他 CLI 為 undefined */
  outputFile?: string;
}

/** codex 可關閉的工具類功能(`codex features list` 實測的名稱) */
const CODEX_DISABLED_FEATURES = [
  'shell_tool',
  'unified_exec',
  'apps',
  'browser_use',
  'browser_use_external',
  'computer_use',
  'in_app_browser',
  'plugins',
  'image_generation',
  'memories',
  'goals',
];

/** gemini 內建工具全部排除,只留純文字生成 */
const GEMINI_EXCLUDED_TOOLS = [
  'run_shell_command',
  'read_file',
  'read_many_files',
  'write_file',
  'replace',
  'glob',
  'list_directory',
  'search_file_content',
  'grep',
  'web_fetch',
  'google_web_search',
  'save_memory',
  'write_todos',
];

/**
 * 組出關閉所有工具的無頭命令:
 * - claude:-p --tools "" --strict-mcp-config --setting-sources "" --no-session-persistence --output-format json --json-schema <schema> --system-prompt <system>
 * - codex:exec --sandbox read-only --ephemeral --skip-git-repo-check --ignore-user-config --ignore-rules
 *          -c features.shell_tool=false(及 unified_exec/apps/browser_use/computer_use/plugins…=false)--output-schema <file> -o <file> -
 * - gemini:-o json -e none,工具排除清單寫在 workDir/.gemini/settings.json
 */
export function buildCliInvocation(kind: CliKind, opts: CliBuildOptions): CliInvocation {
  const model = opts.model ? ['--model', opts.model] : [];
  if (kind === 'claude') {
    return {
      command: 'claude',
      args: [
        '-p',
        '--tools',
        '',
        '--strict-mcp-config',
        '--setting-sources',
        '',
        '--no-session-persistence',
        '--output-format',
        'json',
        '--json-schema',
        JSON.stringify(TRANSLATIONS_SCHEMA),
        '--system-prompt',
        opts.systemPrompt,
        ...model,
      ],
      stdin: opts.payload,
    };
  }
  if (kind === 'codex') {
    const schemaFile = path.join(opts.workDir, 'schema.json');
    const outputFile = path.join(opts.workDir, 'last-message.txt');
    writeFileSync(schemaFile, JSON.stringify(TRANSLATIONS_SCHEMA));
    return {
      command: 'codex',
      args: [
        'exec',
        '--sandbox',
        'read-only',
        '--ephemeral',
        '--skip-git-repo-check',
        '--ignore-user-config',
        '--ignore-rules',
        ...CODEX_DISABLED_FEATURES.flatMap((f) => ['-c', `features.${f}=false`]),
        '-c',
        'web_search="disabled"',
        '-c',
        'model_reasoning_effort="low"',
        ...(opts.model ? ['-m', opts.model] : []),
        '--output-schema',
        schemaFile,
        '-o',
        outputFile,
        '-',
      ],
      // codex 沒有獨立的 system prompt 參數,指示與段落一起走 stdin
      stdin: `${opts.systemPrompt}\n\nINPUT:\n${opts.payload}`,
      outputFile,
    };
  }
  const settingsDir = path.join(opts.workDir, '.gemini');
  mkdirSync(settingsDir, { recursive: true });
  writeFileSync(path.join(settingsDir, 'settings.json'), JSON.stringify({ tools: { exclude: GEMINI_EXCLUDED_TOOLS } }));
  return {
    command: 'gemini',
    args: ['-o', 'json', '-e', 'none', '--approval-mode', 'default', ...model],
    stdin: `${opts.systemPrompt}\n\nINPUT:\n${opts.payload}`,
  };
}
