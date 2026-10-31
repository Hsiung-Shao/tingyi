import { loadSettings, saveSettings, type Settings } from '../shared/settings.ts';

/** 本機程式登錄的 native messaging host 名稱 */
export const HOST_NAME = 'com.jerry.tingyi.host';

/** 本地模型狀態(與本機程式 runtime.json 的 status 相同) */
export type ModelStatus = 'ready' | 'downloading' | 'starting' | 'stopped' | 'not-downloaded' | 'error';

/** 本機程式對 {type:'hello'} 的回覆 */
export type HostReply =
  | {
      ok: true;
      status: ModelStatus;
      llama: { baseUrl: string; model: string; apiKey: string } | null;
      /** 網頁翻譯可用的 llama-server slot(聽譯的共用伺服器才有;字幕固定用其他 slot) */
      slots?: number[];
      bridge: { url: string; token: string } | null;
      message?: string | null;
      appVersion: string;
    }
  | { ok: false; error: string; message?: string };

export type LinkStatus =
  /** 連上本機程式;status 是本地模型狀態 */
  | { state: 'connected'; status: ModelStatus; message?: string }
  /** 沒有安裝本機程式(找不到 native messaging host) */
  | { state: 'not-installed' }
  /** 使用者選擇手動設定,沒有連線 */
  | { state: 'manual' }
  | { state: 'app-error'; message: string };

/**
 * 把本機程式的回覆合併進設定(不改 manual 模式):
 * - llama 有值 → direct 的 baseUrl/model/apiKey,format 'single'、jsonMode false
 * - bridge 有值 → bridge 的 url/token
 * - 第一次連上(linked=false)把 route 設成 'direct' 並記 linked=true;之後不動 route
 */
export function applyHostReply(s: Settings, reply: HostReply): Settings {
  if (s.connection === 'manual' || !reply.ok) return s;
  const next: Settings = { ...s, direct: { ...s.direct }, bridge: { ...s.bridge } };
  if (reply.llama) {
    next.direct.baseUrl = reply.llama.baseUrl;
    next.direct.model = reply.llama.model;
    next.direct.apiKey = reply.llama.apiKey;
    // 舊版本機程式沒有 slot 分工
    next.direct.slots = reply.slots ?? [];
    // 本地翻譯專用小模型一次只送一段,也不要求 JSON 格式
    next.direct.format = 'single';
    next.direct.jsonMode = false;
  }
  if (reply.bridge) {
    next.bridge.url = reply.bridge.url;
    next.bridge.token = reply.bridge.token;
  }
  if (!s.linked) {
    next.route = 'direct';
    next.linked = true;
  }
  return next;
}

/** 送 hello 給本機程式並更新設定;send 預設 chrome.runtime.sendNativeMessage */
export async function autoConnect(
  send: (host: string, msg: unknown) => Promise<unknown> = (host, msg) => chrome.runtime.sendNativeMessage(host, msg as object),
): Promise<LinkStatus> {
  const s = await loadSettings();
  if (s.connection === 'manual') return { state: 'manual' };
  let reply: HostReply;
  try {
    reply = (await send(HOST_NAME, { type: 'hello' })) as HostReply;
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    // Chrome 找不到登錄的 host 時的固定訊息
    if (/native messaging host not found/i.test(message)) return { state: 'not-installed' };
    return { state: 'app-error', message };
  }
  if (!reply?.ok) return { state: 'app-error', message: reply?.message ?? reply?.error ?? '本機程式回應格式不對' };
  await saveSettings(applyHostReply(s, reply));
  return { state: 'connected', status: reply.status, ...(reply.message ? { message: reply.message } : {}) };
}
