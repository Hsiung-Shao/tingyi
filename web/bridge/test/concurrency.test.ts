import { afterEach, describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { createBridgeServer, type BridgeHandle } from '../src/server.ts';

let handle: BridgeHandle | undefined;
afterEach(async () => {
  await handle?.close();
  handle = undefined;
});

describe('bridge 併發', () => {
  it('同一後端同時最多跑 2 個 CLI,其餘在 bridge 內排隊', async () => {
    let running = 0;
    let peak = 0;
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
            running++;
            peak = Math.max(peak, running);
            await new Promise((r) => setTimeout(r, 200));
            running--;
            return JSON.stringify({ translations: req.segments.map((s) => ({ id: s.id, text: 't' })) });
          },
        },
      ],
    });
    const call = async (id: number) => {
      const client = new Client({ name: 't', version: '0' });
      await client.connect(new StreamableHTTPClientTransport(new URL(handle!.url), { requestInit: { headers: { Authorization: 'Bearer tok' } } }));
      const res = await client.callTool({
        name: 'translate_batch',
        arguments: { provider: 'slow', targetLang: 'zh-TW', segments: [{ id, text: 'x' }] },
      });
      await client.close();
      return res.structuredContent as { missing: number[] };
    };
    const results = await Promise.all([1, 2, 3, 4, 5].map(call));
    expect(peak).toBe(2);
    expect(results.every((r) => r.missing.length === 0)).toBe(true);
  });
});
