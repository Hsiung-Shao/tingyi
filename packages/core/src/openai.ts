import type { PromptPair } from './types.ts';

export interface OpenAICompatibleConfig {
  /** 例:http://127.0.0.1:11434/v1(不含 /chat/completions) */
  baseUrl: string;
  model: string;
  apiKey?: string;
  /** 送 response_format: {type:'json_object'} */
  jsonMode?: boolean;
  /** 合併進 request body,例 {keep_alive:'5m'} */
  extraBody?: Record<string, unknown>;
  timeoutMs?: number;
  signal?: AbortSignal;
}

/** POST {baseUrl}/chat/completions,回傳 choices[0].message.content;非 2xx 時 throw 含狀態碼的 Error */
export async function callOpenAICompatible(
  cfg: OpenAICompatibleConfig,
  prompt: PromptPair,
  fetchImpl: typeof fetch = fetch,
): Promise<string> {
  const messages = [
    ...(prompt.system ? [{ role: 'system', content: prompt.system }] : []),
    { role: 'user', content: prompt.user },
  ];
  const body: Record<string, unknown> = {
    ...cfg.extraBody,
    model: cfg.model,
    messages,
    temperature: 0,
    stream: false,
    ...(cfg.jsonMode ? { response_format: { type: 'json_object' } } : {}),
  };
  const res = await fetchImpl(`${cfg.baseUrl.replace(/\/$/, '')}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(cfg.apiKey ? { Authorization: `Bearer ${cfg.apiKey}` } : {}),
    },
    body: JSON.stringify(body),
    signal: AbortSignal.any([AbortSignal.timeout(cfg.timeoutMs ?? 120_000), ...(cfg.signal ? [cfg.signal] : [])]),
  });
  if (!res.ok) {
    const detail = (await res.text().catch(() => '')).slice(0, 300);
    throw new Error(`OpenAI 相容端點回應 ${res.status}${detail ? `:${detail}` : ''}`);
  }
  const data = (await res.json()) as { choices?: { message?: { content?: string } }[] };
  return data.choices?.[0]?.message?.content ?? '';
}
