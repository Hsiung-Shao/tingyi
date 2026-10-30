import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

let cached: { key: string; client: Promise<Client> } | null = null;

async function connect(url: string, token: string): Promise<Client> {
  const client = new Client({ name: 'agent-translator-extension', version: '0.1.0' });
  const transport = new StreamableHTTPClientTransport(new URL(url), {
    requestInit: { headers: { Authorization: `Bearer ${token}` } },
  });
  await client.connect(transport);
  return client;
}

function getClient(url: string, token: string): Promise<Client> {
  const key = `${url}\n${token}`;
  if (cached?.key !== key) cached = { key, client: connect(url, token) };
  return cached.client;
}

/** 呼叫 bridge 的 MCP 工具;連線失敗時丟掉快取的 client 重連一次(SW 可能被回收過) */
export async function callBridgeTool<T>(url: string, token: string, name: string, args: Record<string, unknown>): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      const client = await getClient(url, token);
      const res = await client.callTool({ name, arguments: args });
      if (res.isError) {
        const content = res.content as { type: string; text?: string }[] | undefined;
        throw new BridgeToolError(content?.map((c) => c.text ?? '').join('\n') || `${name} 失敗`);
      }
      if (res.structuredContent) return res.structuredContent as T;
      const first = (res.content as { type: string; text?: string }[])[0];
      return JSON.parse(first?.text ?? '{}') as T;
    } catch (e) {
      if (e instanceof BridgeToolError || attempt >= 1) throw e;
      cached = null;
    }
  }
}

class BridgeToolError extends Error {}
