import { spawn, spawnSync } from 'node:child_process';

export interface RunOptions {
  command: string;
  args: string[];
  stdin: string;
  cwd: string;
  env?: NodeJS.ProcessEnv;
  timeoutMs: number;
  /** 呼叫端放棄(例如 MCP 客戶端斷線)時終止子程序樹 */
  signal?: AbortSignal;
  onSpawn?: (pid: number) => void;
}

export interface RunResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
}

export class ProcessTimeoutError extends Error {
  readonly pid: number | undefined;
  constructor(pid: number | undefined, timeoutMs: number) {
    super(`process timed out after ${timeoutMs} ms`);
    this.name = 'ProcessTimeoutError';
    this.pid = pid;
  }
}

function killTree(pid: number): void {
  if (process.platform === 'win32') {
    // /T 連子程序一起結束;結果由呼叫端以 exit 事件確認,不只信 taskkill 的回傳
    spawnSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true });
  } else {
    try {
      process.kill(-pid, 'SIGKILL');
    } catch {
      process.kill(pid, 'SIGKILL');
    }
  }
}

/** 不經 shell 啟動子程序、stdin 寫入後關閉;逾時終止整個子程序樹並以 ProcessTimeoutError reject */
export function runProcess(opts: RunOptions): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(opts.command, opts.args, {
      cwd: opts.cwd,
      env: opts.env,
      shell: false,
      windowsHide: true,
      detached: process.platform !== 'win32',
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    if (child.pid !== undefined) opts.onSpawn?.(child.pid);
    let stdout = '';
    let stderr = '';
    let timedOut = false;
    child.stdout.setEncoding('utf8').on('data', (d: string) => (stdout += d));
    child.stderr.setEncoding('utf8').on('data', (d: string) => (stderr += d));
    child.stdin.on('error', () => {}); // 子程序提早結束時寫 stdin 會 EPIPE,交給 exit 處理
    child.stdin.end(opts.stdin, 'utf8');

    const stop = () => {
      if (child.pid !== undefined) killTree(child.pid);
      else child.kill('SIGKILL');
    };
    let aborted = false;
    const onAbort = () => {
      aborted = true;
      stop();
    };
    const timer = setTimeout(() => {
      timedOut = true;
      stop();
    }, opts.timeoutMs);
    if (opts.signal?.aborted) onAbort();
    else opts.signal?.addEventListener('abort', onAbort, { once: true });
    const cleanup = () => {
      clearTimeout(timer);
      opts.signal?.removeEventListener('abort', onAbort);
    };

    child.on('error', (e) => {
      cleanup();
      reject(e);
    });
    // close 在行程結束且 stdio 關閉後才觸發,這時行程已確定不在了
    child.on('close', (code) => {
      cleanup();
      if (aborted) reject(new Error('process aborted by caller'));
      else if (timedOut) reject(new ProcessTimeoutError(child.pid, opts.timeoutMs));
      else resolve({ stdout, stderr, exitCode: code });
    });
  });
}
