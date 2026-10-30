import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { callOpenAICompatible } from '../src/openai.ts';

interface Captured {
  url: string;
  method: string;
  headers: IncomingMessage['headers'];
  body: Record<string, unknown>;
}

let server: Server;
let base: string;
let last = undefined as Captured | undefined;
const reset = (): void => {
  last = undefined;
};
let nextStatus = 200;

beforeAll(async () => {
  server = createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => (raw += c));
    req.on('end', () => {
      last = {
        url: req.url ?? '',
        method: req.method ?? '',
        headers: req.headers,
        body: raw ? (JSON.parse(raw) as Record<string, unknown>) : {},
      };
      res.writeHead(nextStatus, { 'Content-Type': 'application/json' });
      if (nextStatus >= 400) res.end(JSON.stringify({ error: 'boom' }));
      else res.end(JSON.stringify({ choices: [{ message: { content: '你好世界' } }] }));
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
});

afterAll(async () => {
  await new Promise<void>((r) => server.close(() => r()));
});

const prompt = { system: 'SYS', user: 'USR' };

describe('callOpenAICompatible', () => {
  it('T16 POST 到 /chat/completions 並回傳 choices[0].message.content', async () => {
    nextStatus = 200;
    reset();
    const out = await callOpenAICompatible({ baseUrl: base, model: 'm1' }, prompt);
    expect(out).toBe('你好世界');
    expect(last?.method).toBe('POST');
    expect(last?.url).toBe('/v1/chat/completions');
  });

  it('T16 body 帶 model 且 messages 依序為 system、user', async () => {
    nextStatus = 200;
    reset();
    await callOpenAICompatible({ baseUrl: base, model: 'm1' }, prompt);
    expect(last?.body.model).toBe('m1');
    expect(last?.body.messages).toEqual([
      { role: 'system', content: 'SYS' },
      { role: 'user', content: 'USR' },
    ]);
  });

  it('T16 jsonMode 時帶 response_format json_object', async () => {
    nextStatus = 200;
    reset();
    await callOpenAICompatible({ baseUrl: base, model: 'm1', jsonMode: true }, prompt);
    expect(last?.body.response_format).toEqual({ type: 'json_object' });
  });

  it('T16 未開 jsonMode 時不帶 response_format', async () => {
    nextStatus = 200;
    reset();
    await callOpenAICompatible({ baseUrl: base, model: 'm1' }, prompt);
    expect(last).toBeDefined();
    expect(last?.body).not.toHaveProperty('response_format');
  });

  it('T16 有 apiKey 時帶 Bearer 授權標頭,沒有則不帶', async () => {
    nextStatus = 200;
    reset();
    await callOpenAICompatible({ baseUrl: base, model: 'm1', apiKey: 'sk-test' }, prompt);
    expect(last?.headers.authorization).toBe('Bearer sk-test');
    reset();
    await callOpenAICompatible({ baseUrl: base, model: 'm1' }, prompt);
    expect(last).toBeDefined();
    expect(last?.headers.authorization).toBeUndefined();
  });

  it('T16 extraBody 被合併進 request body 且不蓋掉 model', async () => {
    nextStatus = 200;
    reset();
    await callOpenAICompatible({ baseUrl: base, model: 'm1', extraBody: { keep_alive: '5m' } }, prompt);
    expect(last?.body.keep_alive).toBe('5m');
    expect(last?.body.model).toBe('m1');
  });

  it('T16 伺服器回 500 時 throw 且訊息含 500', async () => {
    nextStatus = 500;
    try {
      await expect(callOpenAICompatible({ baseUrl: base, model: 'm1' }, prompt)).rejects.toThrow(/500/);
    } finally {
      nextStatus = 200;
    }
  });
});
