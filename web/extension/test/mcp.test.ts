import { afterEach, describe, expect, it } from 'vitest';
import { createBridgeServer, type BridgeHandle } from '../../bridge/src/server.ts';
import { callBridgeTool } from '../src/background/mcp.ts';

let handle: BridgeHandle | undefined;
afterEach(async () => {
  await handle?.close();
  handle = undefined;
});

describe('callBridgeTool', () => {
  it('換 client 時不會中止另一批還在進行中的請求', async () => {
    handle = await createBridgeServer({
      port: 0,
      token: 'tok',
      allowedOrigins: [],
      providers: [
        {
          id: 'slow',
          label: 'slow',
          limits: { maxSegments: 20, maxChars: 6000 },
          detect: async () => ({ available: true }),
          translate: async (req) => {
            await new Promise((r) => setTimeout(r, 600));
            return JSON.stringify({ translations: req.segments.map((s) => ({ id: s.id, text: `譯${s.id}` })) });
          },
        },
      ],
    });
    const args = { provider: 'slow', targetLang: 'zh-TW', segments: [{ id: 1, text: 'one' }] };
    const inFlight = callBridgeTool<{ translations: { id: number; text: string }[] }>(handle.url, 'tok', 'translate_batch', args);
    await new Promise((r) => setTimeout(r, 150));
    // 設定改變(token 不同)會換新 client;這個請求會被 401 拒絕
    await expect(callBridgeTool(handle.url, 'other-token', 'list_providers', {})).rejects.toThrow();
    const res = await inFlight;
    expect(res.translations).toEqual([{ id: 1, text: '譯1' }]);
  });
});
