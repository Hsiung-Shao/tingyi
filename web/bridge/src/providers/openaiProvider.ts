import { buildPrompt, callOpenAICompatible, type BatchFormat, type OpenAICompatibleConfig } from '@atx/core';
import type { Provider, TranslateRequest } from './types.ts';

export interface OpenAIProviderOptions extends OpenAICompatibleConfig {
  id?: string;
  label?: string;
  format?: BatchFormat;
}

export function createOpenAIProvider(opts: OpenAIProviderOptions): Provider {
  const format = opts.format ?? 'json';
  return {
    id: opts.id ?? 'openai-compatible',
    label: opts.label ?? `OpenAI 相容端點(${opts.baseUrl})`,
    format,
    limits: format === 'single' ? { maxSegments: 1, maxChars: 4000 } : { maxSegments: 8, maxChars: 2500 },
    async detect() {
      try {
        const res = await fetch(`${opts.baseUrl.replace(/\/$/, '')}/models`, {
          headers: opts.apiKey ? { Authorization: `Bearer ${opts.apiKey}` } : {},
          signal: AbortSignal.timeout(5000),
        });
        return { available: res.ok, note: res.ok ? undefined : `HTTP ${res.status}` };
      } catch (e) {
        return { available: false, note: String(e) };
      }
    },
    async translate(req: TranslateRequest) {
      const prompt = buildPrompt(req.segments, req.targetLang, format, req.context);
      return callOpenAICompatible({ ...opts, model: req.model ?? opts.model, signal: req.signal }, prompt);
    },
  };
}
