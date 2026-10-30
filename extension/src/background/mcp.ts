import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { McpError } from '@modelcontextprotocol/sdk/types.js';

/** CLI agent 一批 20 段加缺段重試可能好幾分鐘;SDK 預設 60 秒會在 bridge 還在跑時放棄 */
const TOOL_TIMEOUT_MS = 10 * 60_000;

let cached: { key: string; client: Promise<Client> } | null = null;

async function connect(url: string, token: string): Promise<Client> {
  const client = new Client({ name: 'agent-translator-extension', version: '0.1.0' });
  const transport = new StreamableHTTPClientTransport(new URL(url), {
    requestInit: { headers: { Authorization: `Bearer ${token}` } },
  });
  await client.connect(transport);
  return client;
}

/**
 * 只丟掉快取,不 close():transport 的 close 會 abort 所有共用它的 POST,
 * 會把另一批還在跑的請求一起中止。bridge 是 stateless,舊 client 沒有常駐連線,交給 GC。
 */
function dropClient(): void {
  cached = null;
}

function getClient(url: string, token: string): Promise<Client> {
  const key = `${url}\n${token}`;
  if (cached?.key !== key) {
    dropClient();
    cached = { key, client: connect(url, token) };
  }
  return cached.client;
}

/**
 * 呼叫 bridge 的 MCP 工具。只有連線層的錯誤(SW 被回收、bridge 重啟)才重連重送一次;
 * McpError(逾時、工具錯誤)代表請求已送達 bridge,重送會讓同一批 CLI 跑兩次。
 */
export async function callBridgeTool<T>(url: string, token: string, name: string, args: Record<string, unknown>): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      const client = await getClient(url, token);
      const res = await client.callTool({ name, arguments: args }, undefined, { timeout: TOOL_TIMEOUT_MS });
      if (res.isError) {
        const content = res.content as { type: string; text?: string }[] | undefined;
        throw new BridgeToolError(content?.map((c) => c.text ?? '').join('\n') || `${name} 失敗`);
      }
      if (res.structuredContent) return res.structuredContent as T;
      const first = (res.content as { type: string; text?: string }[])[0];
      return JSON.parse(first?.text ?? '{}') as T;
    } catch (e) {
      if (e instanceof BridgeToolError || e instanceof McpError || attempt >= 1) throw e;
      dropClient();
    }
  }
}

class BridgeToolError extends Error {}
