import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

export interface ResolvedCommand {
  command: string;
  /** 放在 CLI 自己參數之前(npm shim 改由 node 執行 JS 入口時為 [entry.js]) */
  prefixArgs: string[];
}

/**
 * 在 PATH 找可執行檔。Windows 的 .cmd shim 不能不經 shell 直接 spawn,
 * 所以解析 shim 內的 JS 入口路徑,改成 node <entry.js>。
 */
export function resolveCommand(name: string): ResolvedCommand | null {
  const dirs = (process.env.PATH ?? '').split(path.delimiter).filter(Boolean);
  const isWin = process.platform === 'win32';
  for (const dir of dirs) {
    if (isWin) {
      const exe = path.join(dir, `${name}.exe`);
      if (existsSync(exe)) return { command: exe, prefixArgs: [] };
      const shim = path.join(dir, `${name}.cmd`);
      if (existsSync(shim)) {
        // npm 用 "%dp0%\...",pnpm/yarn 用 "%~dp0\..\..."
        const m = /"%(?:dp0%|~dp0)\\([^"]+\.js)"/.exec(readFileSync(shim, 'utf8'));
        if (m?.[1]) {
          const entry = path.resolve(dir, m[1]);
          if (existsSync(entry)) return { command: process.execPath, prefixArgs: [entry] };
        }
      }
    } else {
      const bin = path.join(dir, name);
      if (existsSync(bin)) return { command: bin, prefixArgs: [] };
    }
  }
  return null;
}

/** 移除宿主 Claude Code session 注入的變數,避免子程序的 claude 連到宿主的認證與代理 */
export function sanitizedEnv(env: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const out: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(env)) {
    if (k === 'CLAUDECODE' || k.startsWith('CLAUDE_CODE_')) continue;
    out[k] = v;
  }
  return out;
}
