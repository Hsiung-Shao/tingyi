import { afterEach, describe, expect, it } from 'vitest';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { resolveCommand } from '../src/resolve.ts';

const dirs: string[] = [];
const oldPath = process.env.PATH;
afterEach(() => {
  process.env.PATH = oldPath;
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

describe.runIf(process.platform === 'win32')('resolveCommand Windows shim', () => {
  it('npm 的 %dp0% shim 解析成 node 加 JS 入口', () => {
    const bin = mkdtempSync(path.join(tmpdir(), 'atx-bin-'));
    dirs.push(bin);
    mkdirSync(path.join(bin, 'node_modules', 'fake', 'bin'), { recursive: true });
    writeFileSync(path.join(bin, 'node_modules', 'fake', 'bin', 'cli.js'), '');
    writeFileSync(path.join(bin, 'fakecli.cmd'), '@"%_prog%"  "%dp0%\\node_modules\\fake\\bin\\cli.js" %*\r\n');
    process.env.PATH = bin;
    const r = resolveCommand('fakecli');
    expect(r?.command).toBe(process.execPath);
    expect(r?.prefixArgs).toEqual([path.join(bin, 'node_modules', 'fake', 'bin', 'cli.js')]);
  });

  it('pnpm 的 %~dp0 加上層目錄的 shim 也能解析', () => {
    const root = mkdtempSync(path.join(tmpdir(), 'atx-pnpm-'));
    dirs.push(root);
    const bin = path.join(root, 'bin');
    mkdirSync(path.join(root, 'global', 'node_modules', 'fake'), { recursive: true });
    mkdirSync(bin);
    writeFileSync(path.join(root, 'global', 'node_modules', 'fake', 'cli.js'), '');
    writeFileSync(path.join(bin, 'fakecli.cmd'), '@"%~dp0\\..\\global\\node_modules\\fake\\cli.js" %*\r\n');
    process.env.PATH = bin;
    const r = resolveCommand('fakecli');
    expect(r?.prefixArgs).toEqual([path.join(root, 'global', 'node_modules', 'fake', 'cli.js')]);
  });
});
