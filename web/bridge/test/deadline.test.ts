import { afterEach, describe, expect, it } from 'vitest';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { createBridgeServer, type BridgeHandle } from '../src/server.ts';

let handle: BridgeHandle | undefined;
afterEach(async () => {
  await handle?.close();
  handle = undefined;
});

describe('translate_batch 期限', () => {
  it('超過期限就中止後端並回傳 missing,不會拖過瀏覽器的請求上限', async () => {
    let aborted = false;
    handle = await createBridgeServer({
      port: 0,
      token: 'tok',
      allowedOrigins: [],
      deadlineMs: 300,
      providers: [
        {
          id: 'slow',
          label: 'slow',
          limits: { maxSegments: 20, maxChars: 6000 },
          detect: async () => ({ available: true }),
          translate: (req) =>
            new Promise((_, reject) => {
              const t = setTimeout(() => reject(new Error('should not finish')), 5000);
              req.signal?.addEventListener('abort', () => {
                aborted = true;
                clearTimeout(t);
                reject(new Error('aborted'));
              });
            }),
        },
      ],
    });
    const client = new Client({ name: 't', version: '0' });
    await client.connect(new StreamableHTTPClientTransport(new URL(handle.url), { requestInit: { headers: { Authorization: 'Bearer tok' } } }));
    const started = Date.now();
    const res = await client.callTool({
      name: 'translate_batch',
      arguments: { provider: 'slow', targetLang: 'zh-TW', segments: [{ id: 1, text: 'one' }] },
    });
    await client.close();
    expect(Date.now() - started).toBeLessThan(2000);
    expect(aborted).toBe(true);
    expect((res.structuredContent as { missing: number[] }).missing).toEqual([1]);
  });
});
