import { describe, expect, it } from 'vitest';
import { tmpdir } from 'node:os';
import { ProcessTimeoutError, runProcess } from '../src/process.ts';

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function waitUntilGone(pid: number, withinMs: number): Promise<boolean> {
  const deadline = Date.now() + withinMs;
  while (Date.now() < deadline) {
    if (!isAlive(pid)) return true;
    await new Promise((r) => setTimeout(r, 50));
  }
  return !isAlive(pid);
}

describe('runProcess 中止', () => {
  it('呼叫端中止時回錯誤且子程序已消失', async () => {
    let pid: number | undefined;
    const ctrl = new AbortController();
    const run = runProcess({
      command: process.execPath,
      args: ['-e', 'setInterval(()=>{},1000)'],
      stdin: '',
      cwd: tmpdir(),
      timeoutMs: 30_000,
      signal: ctrl.signal,
      onSpawn: (p) => (pid = p),
    });
    setTimeout(() => ctrl.abort(), 300);
    await expect(run).rejects.toThrow(/abort/i);
    expect(pid).toBeDefined();
    expect(await waitUntilGone(pid!, 2000)).toBe(true);
  });
});

describe('runProcess', () => {
  it('T15 假 CLI 一直不結束時回逾時錯誤且行程已消失', async () => {
    let pid: number | undefined;
    const started = Date.now();
    await expect(
      runProcess({
        command: process.execPath,
        args: ['-e', 'setInterval(()=>{},1000)'],
        stdin: '',
        cwd: tmpdir(),
        timeoutMs: 500,
        onSpawn: (p) => {
          pid = p;
        },
      }),
    ).rejects.toBeInstanceOf(ProcessTimeoutError);
    expect(Date.now() - started).toBeLessThan(10000);
    expect(typeof pid).toBe('number');
    expect(await waitUntilGone(pid as number, 2000)).toBe(true);
  });

  it('T15 正常結束的行程回傳 stdout 與 exitCode 0', async () => {
    const r = await runProcess({
      command: process.execPath,
      args: ['-e', 'process.stdin.pipe(process.stdout)'],
      stdin: 'hello-stdin',
      cwd: tmpdir(),
      timeoutMs: 10000,
    });
    expect(r.stdout).toBe('hello-stdin');
    expect(r.exitCode).toBe(0);
  });

  it('T15 非零結束碼會原樣回傳並帶 stderr', async () => {
    const r = await runProcess({
      command: process.execPath,
      args: ['-e', 'console.error("oops");process.exit(3)'],
      stdin: '',
      cwd: tmpdir(),
      timeoutMs: 10000,
    });
    expect(r.exitCode).toBe(3);
    expect(r.stderr).toContain('oops');
  });
});
