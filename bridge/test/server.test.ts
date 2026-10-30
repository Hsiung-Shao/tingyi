import { afterEach, describe, expect, it } from 'vitest';
import { request } from 'node:http';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { createBridgeServer, type BridgeHandle } from '../src/server.ts';
import type { Provider, TranslateRequest } from '../src/providers/types.ts';

const TOKEN = 'test-token-123';
const EXT_ORIGIN = 'chrome-extension://abc';

const handles: BridgeHandle[] = [];
const clients: Client[] = [];

afterEach(async () => {
  for (const c of clients.splice(0)) await c.close().catch(() => {});
  for (const h of handles.splice(0)) await h.close().catch(() => {});
});

function mockProvider(translate?: (req: TranslateRequest) => Promise<string>): Provider {
  return {
    id: 'mock',
    label: 'Mock Provider',
    limits: { maxSegments: 20, maxChars: 6000 },
    detect: async () => ({ available: true, version: '9.9.9' }),
    translate: translate ?? (async () => JSON.stringify({ translations: [] })),
  };
}

async function startBridge(providers: Provider[]): Promise<BridgeHandle> {
  const handle = await createBridgeServer({
    port: 0,
    token: TOKEN,
    allowedOrigins: [EXT_ORIGIN],
    providers,
  });
  handles.push(handle);
  expect(handle.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  return handle;
}

/** 送一個 initialize JSON-RPC POST,回傳狀態碼;可自訂標頭(含 Host) */
function postInitialize(url: string, headers: Record<string, string>): Promise<number> {
  const u = new URL(url);
  const body = JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'initialize',
    params: {
      protocolVersion: '2025-03-26',
      capabilities: {},
      clientInfo: { name: 'raw-test', version: '0.0.0' },
    },
  });
  return new Promise((resolve, reject) => {
    const req = request(
      {
        host: u.hostname,
        port: u.port,
        path: u.pathname,
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Accept: 'application/json, text/event-stream',
          'Content-Length': Buffer.byteLength(body),
          ...headers,
        },
      },
      (res) => {
        const status = res.statusCode ?? 0;
        res.destroy();
        resolve(status);
      },
    );
    req.on('error', reject);
    req.end(body);
  });
}

async function connect(handle: BridgeHandle): Promise<Client> {
  const client = new Client({ name: 'test-client', version: '0.0.0' });
  const transport = new StreamableHTTPClientTransport(new URL(handle.url), {
    requestInit: { headers: { Authorization: `Bearer ${TOKEN}` } },
  });
  clients.push(client);
  await client.connect(transport);
  return client;
}

/** 取工具結果:優先 structuredContent,否則解析 content[0].text 的 JSON */
function payloadOf(result: unknown): Record<string, unknown> {
  const r = result as {
    structuredContent?: Record<string, unknown>;
    content?: Array<{ type: string; text?: string }>;
  };
  if (r.structuredContent) return r.structuredContent;
  const text = r.content?.[0]?.text;
  return JSON.parse(text ?? 'null') as Record<string, unknown>;
}

describe('bridge 驗證', () => {
  it('T11 沒帶 token 的請求回 401', async () => {
    const h = await startBridge([mockProvider()]);
    const status = await postInitialize(h.url, { Origin: EXT_ORIGIN });
    expect(status).toBe(401);
  });

  it('T11 token 錯誤的請求回 401', async () => {
    const h = await startBridge([mockProvider()]);
    const status = await postInitialize(h.url, { Authorization: 'Bearer wrong-token' });
    expect(status).toBe(401);
  });

  it('T12 Origin 是 evil.com 時即使 token 正確也回 403', async () => {
    const h = await startBridge([mockProvider()]);
    const status = await postInitialize(h.url, {
      Authorization: `Bearer ${TOKEN}`,
      Origin: 'https://evil.com',
    });
    expect(status).toBe(403);
  });

  it('T12 Host 是 evil.com 時即使 token 正確也回 403', async () => {
    const h = await startBridge([mockProvider()]);
    const status = await postInitialize(h.url, {
      Authorization: `Bearer ${TOKEN}`,
      Host: 'evil.com',
    });
    expect(status).toBe(403);
  });

  it('T12 對照:正確 token 加允許的 Origin 不是 401 也不是 403', async () => {
    const h = await startBridge([mockProvider()]);
    const status = await postInitialize(h.url, {
      Authorization: `Bearer ${TOKEN}`,
      Origin: EXT_ORIGIN,
    });
    expect(status).not.toBe(401);
    expect(status).not.toBe(403);
    expect(status).toBeLessThan(300);
  });
});

describe('bridge MCP 工具', () => {
  it('T13 列出 list_providers 與 translate_batch 兩個工具', async () => {
    const h = await startBridge([mockProvider()]);
    const client = await connect(h);
    const { tools } = await client.listTools();
    const names = tools.map((t) => t.name);
    expect(names).toContain('list_providers');
    expect(names).toContain('translate_batch');
  });

  it('T13 list_providers 回傳 mock provider 的資訊', async () => {
    const h = await startBridge([mockProvider()]);
    const client = await connect(h);
    const data = payloadOf(await client.callTool({ name: 'list_providers', arguments: {} }));
    const providers = data.providers as Array<Record<string, unknown>>;
    expect(providers).toHaveLength(1);
    expect(providers[0]).toMatchObject({ id: 'mock', label: 'Mock Provider', available: true, version: '9.9.9' });
  });

  it('T13 translate_batch 依 id 對回譯文並帶 missing 與 elapsedMs', async () => {
    const seen: TranslateRequest[] = [];
    const h = await startBridge([
      mockProvider(async (req) => {
        seen.push(req);
        return JSON.stringify({
          translations: [
            { id: 3, text: '三' },
            { id: 1, text: '一' },
            { id: 2, text: '二' },
          ],
        });
      }),
    ]);
    const client = await connect(h);
    const data = payloadOf(
      await client.callTool({
        name: 'translate_batch',
        arguments: {
          provider: 'mock',
          targetLang: 'zh-TW',
          segments: [
            { id: 1, text: 'one' },
            { id: 2, text: 'two' },
            { id: 3, text: 'three' },
          ],
        },
      }),
    );
    const translations = (data.translations as Array<{ id: number; text: string }>)
      .slice()
      .sort((a, b) => a.id - b.id);
    expect(translations).toEqual([
      { id: 1, text: '一' },
      { id: 2, text: '二' },
      { id: 3, text: '三' },
    ]);
    expect(data.missing).toEqual([]);
    expect(typeof data.elapsedMs).toBe('number');
    expect(seen[0]?.targetLang).toBe('zh-TW');
    expect(seen[0]?.segments.map((s) => s.text)).toEqual(['one', 'two', 'three']);
  });

  it('T13 後端一直缺某段時該 id 列在 missing', async () => {
    const h = await startBridge([
      mockProvider(async (req) =>
        JSON.stringify({
          translations: req.segments.filter((s) => s.id !== 2).map((s) => ({ id: s.id, text: `譯${s.id}` })),
        }),
      ),
    ]);
    const client = await connect(h);
    const data = payloadOf(
      await client.callTool({
        name: 'translate_batch',
        arguments: {
          provider: 'mock',
          targetLang: 'zh-TW',
          segments: [
            { id: 1, text: 'one' },
            { id: 2, text: 'two' },
            { id: 3, text: 'three' },
          ],
        },
      }),
    );
    const ids = (data.translations as Array<{ id: number }>).map((t) => t.id).sort();
    expect(ids).toEqual([1, 3]);
    expect(data.missing).toEqual([2]);
  });
});
