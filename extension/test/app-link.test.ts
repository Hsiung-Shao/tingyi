import { afterEach, describe, expect, it, vi } from 'vitest';
import { autoConnect, HOST_NAME, type HostReply } from '../src/background/appLink.ts';
import { DEFAULT_SETTINGS, SETTINGS_KEY, type Settings } from '../src/shared/settings.ts';

/** 只假造 chrome.storage.local(外部 API),回傳可檢查的 store */
function fakeChrome(initial?: Settings) {
  const store: Record<string, unknown> = initial ? { [SETTINGS_KEY]: structuredClone(initial) } : {};
  vi.stubGlobal('chrome', {
    storage: {
      local: {
        get: async (key: string) => (key in store ? { [key]: structuredClone(store[key]) } : {}),
        set: async (obj: Record<string, unknown>) => {
          for (const [k, v] of Object.entries(obj)) store[k] = structuredClone(v);
        },
      },
    },
  });
  return store;
}

const READY: HostReply = {
  ok: true,
  status: 'ready',
  llama: { baseUrl: 'http://127.0.0.1:51234/v1', model: 'hy-mt2', apiKey: 'key-abc' },
  bridge: { url: 'http://127.0.0.1:47821/mcp', token: 'bridge-token' },
  appVersion: '0.1.0',
};

const sender = (reply: unknown) => vi.fn(async (_host: string, _msg: unknown) => reply);
const saved = (store: Record<string, unknown>) => store[SETTINGS_KEY] as Settings;

afterEach(() => vi.unstubAllGlobals());

describe('擴充自動連線本機程式', () => {
  // E1
  it('連上後把 llama 與 bridge 端點寫進設定', async () => {
    const store = fakeChrome();
    const send = sender(READY);

    const result = await autoConnect(send);

    expect(send).toHaveBeenCalledWith(HOST_NAME, { type: 'hello' }); // 送錯 host 名稱或訊息就連不上
    expect(result).toEqual({ state: 'connected', status: 'ready' });
    const s = saved(store);
    expect(s.direct.baseUrl).toBe('http://127.0.0.1:51234/v1');
    expect(s.direct.model).toBe('hy-mt2');
    expect(s.direct.apiKey).toBe('key-abc');
    expect(s.direct.format).toBe('single'); // 本地小模型一次只送一段
    expect(s.direct.jsonMode).toBe(false); // 預設是 true,沒關會讓 llama 回錯格式
    expect(s.bridge.url).toBe('http://127.0.0.1:47821/mcp');
    expect(s.bridge.token).toBe('bridge-token');
  });

  // E2
  it('手動模式的設定不被覆蓋也不呼叫本機程式', async () => {
    const manual: Settings = {
      ...DEFAULT_SETTINGS,
      connection: 'manual',
      direct: { ...DEFAULT_SETTINGS.direct, baseUrl: 'http://example.test/v1', apiKey: 'my-own-key' },
    };
    const store = fakeChrome(manual);
    const send = sender(READY);

    const result = await autoConnect(send);

    expect(result).toEqual({ state: 'manual' });
    expect(send).not.toHaveBeenCalled();
    expect(saved(store)).toEqual(manual);
  });

  // E3
  it('已連過且選了 bridge 時 route 不被改回 direct', async () => {
    const store = fakeChrome({ ...DEFAULT_SETTINGS, linked: true, route: 'bridge' });

    await autoConnect(sender(READY));

    const s = saved(store);
    expect(s.route).toBe('bridge'); // 使用者之後的選擇不能被每次自動連線蓋掉
    expect(s.linked).toBe(true);
    expect(s.direct.baseUrl).toBe('http://127.0.0.1:51234/v1'); // 端點仍然更新
  });

  // E3
  it('第一次連上時 route 設成 direct 並標記 linked', async () => {
    const store = fakeChrome({ ...DEFAULT_SETTINGS, linked: false, route: 'bridge' });

    await autoConnect(sender(READY));

    const s = saved(store);
    expect(s.route).toBe('direct');
    expect(s.linked).toBe(true);
  });

  // E4
  it('找不到 native host 時回 not-installed 且設定不變', async () => {
    const before: Settings = { ...DEFAULT_SETTINGS, targetLang: 'ja', linked: true };
    const store = fakeChrome(before);
    const send = vi.fn(async () => {
      throw new Error('Specified native messaging host not found.');
    });

    const result = await autoConnect(send);

    expect(result).toEqual({ state: 'not-installed' });
    expect(saved(store)).toEqual(before);
  });
});
