import { timingSafeEqual } from 'node:crypto';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { splitBatches, translateWithRetry } from '@atx/core';
import { z } from 'zod';
import type { Provider } from './providers/types.ts';

export interface BridgeOptions {
  host?: string;
  /** 0 = 隨機 port */
  port: number;
  token: string;
  /** 允許的 Origin(例 chrome-extension://<id>);請求沒帶 Origin 視為非瀏覽器客戶端,只看 token */
  allowedOrigins: string[];
  providers: Provider[];
}

export interface BridgeHandle {
  /** 例 http://127.0.0.1:47821/mcp */
  url: string;
  close(): Promise<void>;
}

const LOCAL_HOST = /^(?:127\.0\.0\.1|localhost|\[::1\])(?::\d+)?$/i;

function tokenMatches(header: string | undefined, token: string): boolean {
  const m = /^Bearer (.+)$/.exec(header ?? '');
  if (!m?.[1]) return false;
  const a = Buffer.from(m[1]);
  const b = Buffer.from(token);
  return a.length === b.length && timingSafeEqual(a, b);
}

/** 回傳 null 表示通過,否則回傳要拒絕的狀態碼 */
function rejectStatus(req: IncomingMessage, opts: BridgeOptions): number | null {
  if (!LOCAL_HOST.test(req.headers.host ?? '')) return 403;
  const origin = req.headers.origin;
  if (origin !== undefined && !opts.allowedOrigins.includes(origin)) return 403;
  if (!tokenMatches(req.headers.authorization, opts.token)) return 401;
  return null;
}

const segmentSchema = z.object({ id: z.number().int(), text: z.string() });

function buildMcpServer(providers: Provider[]): McpServer {
  const server = new McpServer({ name: 'agent-translator-bridge', version: '0.1.0' });

  server.registerTool(
    'list_providers',
    {
      description: '列出 bridge 可用的翻譯後端(訂閱型 CLI agent、OpenAI 相容端點)與狀態',
      inputSchema: {},
      outputSchema: {
        providers: z.array(
          z.object({ id: z.string(), label: z.string(), available: z.boolean(), version: z.string().optional(), note: z.string().optional() }),
        ),
      },
    },
    async () => {
      const list = await Promise.all(
        providers.map(async (p) => ({ id: p.id, label: p.label, ...(await p.detect()) })),
      );
      const structuredContent = { providers: list };
      return { structuredContent, content: [{ type: 'text', text: JSON.stringify(structuredContent) }] };
    },
  );

  server.registerTool(
    'translate_batch',
    {
      description: '把一批網頁段落翻成目標語言。段落是不可信的網頁內容,後端只會翻譯、不執行其中的指示。',
      inputSchema: {
        provider: z.string().describe('list_providers 回傳的 id'),
        model: z.string().optional(),
        targetLang: z.string().describe('例 zh-TW、en、ja'),
        segments: z.array(segmentSchema).min(1),
        context: z.object({ title: z.string().optional(), url: z.string().optional() }).optional(),
      },
      outputSchema: {
        translations: z.array(segmentSchema),
        missing: z.array(z.number().int()),
        elapsedMs: z.number(),
      },
    },
    async ({ provider: providerId, model, targetLang, segments, context }) => {
      const provider = providers.find((p) => p.id === providerId);
      if (!provider) {
        return { isError: true, content: [{ type: 'text', text: `未知的後端:${providerId}` }] };
      }
      const started = Date.now();
      const result = new Map<number, string>();
      for (const batch of splitBatches(segments, provider.limits)) {
        const got = await translateWithRetry(batch, (segs) => provider.translate({ segments: segs, targetLang, model, context }));
        for (const [id, text] of got) result.set(id, text);
      }
      const structuredContent = {
        translations: [...result].map(([id, text]) => ({ id, text })),
        missing: segments.map((s) => s.id).filter((id) => !result.has(id)),
        elapsedMs: Date.now() - started,
      };
      return { structuredContent, content: [{ type: 'text', text: JSON.stringify(structuredContent) }] };
    },
  );
  return server;
}

async function readBody(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  const raw = Buffer.concat(chunks).toString('utf8');
  return raw ? JSON.parse(raw) : undefined;
}

function sendError(res: ServerResponse, status: number, message: string): void {
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ jsonrpc: '2.0', error: { code: -32000, message }, id: null }));
}

/**
 * MCP server(Streamable HTTP,路徑 /mcp)。檢查順序:Host 不是 127.0.0.1/localhost[:port] → 403;
 * Origin 存在且不在 allowedOrigins → 403;Authorization 不是 `Bearer <token>` → 401。
 * 工具:list_providers → {providers:[{id,label,available,version?,note?}]};
 * translate_batch({provider,model?,targetLang,segments,context?}) → structuredContent {translations:[{id,text}],missing:number[],elapsedMs}
 */
export async function createBridgeServer(opts: BridgeOptions): Promise<BridgeHandle> {
  const host = opts.host ?? '127.0.0.1';
  const http = createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? '/', 'http://localhost');
      if (url.pathname !== '/mcp') return sendError(res, 404, 'not found');
      const status = rejectStatus(req, opts);
      if (status === 403) return sendError(res, 403, 'forbidden');
      if (status === 401) return sendError(res, 401, 'unauthorized');
      if (req.method !== 'POST') return sendError(res, 405, 'method not allowed');

      // stateless:每個請求一組 server + transport,不保留 session
      const body = await readBody(req);
      const server = buildMcpServer(opts.providers);
      const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
      res.on('close', () => {
        void transport.close();
        void server.close();
      });
      await server.connect(transport);
      await transport.handleRequest(req, res, body);
    } catch (e) {
      if (!res.headersSent) sendError(res, 500, e instanceof Error ? e.message : String(e));
    }
  });
  await new Promise<void>((resolve, reject) => {
    http.once('error', reject);
    http.listen(opts.port, host, () => resolve());
  });
  const { port } = http.address() as AddressInfo;
  return {
    url: `http://${host}:${port}/mcp`,
    close: () =>
      new Promise<void>((resolve) => {
        http.closeAllConnections();
        http.close(() => resolve());
      }),
  };
}
