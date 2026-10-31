import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart';

type LlamaStatus = 'not-downloaded' | 'downloading' | 'stopped' | 'starting' | 'ready' | 'error';

interface StatusView {
  runtime: {
    status: LlamaStatus;
    llama: { port: number; model: string } | null;
    bridge: { url: string } | null;
    message: string | null;
  };
  llamaDownloaded: boolean;
  modelDownloaded: boolean;
  download: [number, number] | null;
  browsers: [string, boolean][];
  lastHello: number | null;
  node: string | null;
  bridgeEnabled: boolean;
}

interface Progress {
  id: string;
  phase: 'downloading' | 'verifying' | 'done' | 'error' | 'cancelled';
  done: number;
  total: number;
  bps: number;
  message: string | null;
}

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

const STATUS_TEXT: Record<LlamaStatus, [string, '' | 'ok' | 'warn' | 'bad']> = {
  'not-downloaded': ['尚未下載', 'warn'],
  downloading: ['下載中', 'warn'],
  stopped: ['已停止', ''],
  starting: ['啟動中', 'warn'],
  ready: ['執行中', 'ok'],
  error: ['發生錯誤', 'bad'],
};

const mb = (n: number) => `${(n / 1_000_000).toFixed(0)} MB`;

function setBadge(el: HTMLElement, text: string, tone: string): void {
  el.textContent = text;
  el.className = `badge ${tone}`;
}

function render(v: StatusView): void {
  const s = v.runtime.status;
  const [label, tone] = STATUS_TEXT[s];
  setBadge($('model-badge'), label, tone);
  const hint = $('model-hint');
  hint.classList.toggle('bad', s === 'error');
  hint.textContent =
    v.runtime.message ??
    (s === 'ready'
      ? `Hy-MT2 1.8B 正在本機 port ${v.runtime.llama?.port} 提供翻譯。`
      : s === 'not-downloaded'
        ? '第一次使用要下載翻譯引擎與模型,下載完成後會自動啟動。'
        : s === 'starting'
          ? '正在載入模型,第一次約需數十秒。'
          : '');
  $('btn-download').hidden = s !== 'not-downloaded';
  $('btn-cancel').hidden = s !== 'downloading';
  $('btn-start').hidden = !(s === 'stopped' || s === 'error') || !(v.llamaDownloaded && v.modelDownloaded);
  $('btn-stop').hidden = !(s === 'ready' || s === 'starting');
  if (s !== 'downloading') $('progress').hidden = true;

  const list = $('browsers');
  list.replaceChildren(
    ...v.browsers.map(([name, ok]) => {
      const li = document.createElement('li');
      li.textContent = `${name}${ok ? ' 已登錄' : ' 未登錄'}`;
      li.className = ok ? 'ok' : '';
      return li;
    }),
  );
  const linked = v.lastHello !== null;
  setBadge($('link-badge'), linked ? '已連線過' : '尚未連線', linked ? 'ok' : '');
  $('link-hint').textContent = linked
    ? `擴充最近一次連線:${new Date(v.lastHello! * 1000).toLocaleString('zh-TW')}`
    : '安裝 agent-translator 擴充後,開啟擴充的選單就會自動連上這個程式。';

  const bridge = $<HTMLInputElement>('bridge-toggle');
  bridge.checked = v.bridgeEnabled;
  bridge.disabled = !v.node && !v.bridgeEnabled;
  $('bridge-hint').textContent = !v.node
    ? '找不到 Node.js。CLI agent 需要先安裝 Node.js 與 codex 等命令列工具。'
    : v.runtime.bridge
      ? `CLI agent bridge 執行中(${v.runtime.bridge.url})。`
      : '讓已安裝 codex 等命令列 AI 工具的人,也能用它們翻譯。';
}

function renderProgress(p: Progress): void {
  if (p.phase === 'done' || p.phase === 'cancelled' || p.phase === 'error') return;
  $('progress').hidden = false;
  const pct = p.total ? Math.floor((p.done / p.total) * 100) : 0;
  $('progress-fill').style.width = `${pct}%`;
  $('progress-text').textContent =
    p.phase === 'verifying'
      ? `校驗中 ${p.message ?? ''}`
      : `${pct}%  ${mb(p.done)} / ${mb(p.total)}${p.bps ? `  ${(p.bps / 1_000_000).toFixed(1)} MB/s` : ''}`;
}

async function refresh(): Promise<void> {
  render(await invoke<StatusView>('get_status'));
}

$('btn-download').addEventListener('click', () => void invoke('download_start'));
$('btn-cancel').addEventListener('click', () => void invoke('download_cancel'));
$('btn-start').addEventListener('click', () => void invoke('model_start'));
$('btn-stop').addEventListener('click', () => void invoke('model_stop'));
$('btn-logs').addEventListener('click', () => void invoke('open_logs'));
$<HTMLInputElement>('bridge-toggle').addEventListener('change', (e) => {
  void invoke('bridge_set_enabled', { enabled: (e.target as HTMLInputElement).checked });
});
const autostart = $<HTMLInputElement>('autostart-toggle');
autostart.addEventListener('change', () => void (autostart.checked ? enable() : disable()));

void listen('status', () => void refresh());
void listen<Progress>('model-download', (e) => renderProgress(e.payload));
void isEnabled().then((on) => (autostart.checked = on));
void refresh();
// 擴充連線時間由 native host 寫檔,不會發事件,定期重讀
setInterval(() => void refresh(), 5000);
