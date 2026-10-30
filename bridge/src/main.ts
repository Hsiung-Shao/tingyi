import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createCliProvider } from './providers/cliProvider.ts';
import { createOpenAIProvider, type OpenAIProviderOptions } from './providers/openaiProvider.ts';
import type { Provider } from './providers/types.ts';
import { createBridgeServer } from './server.ts';

const EXTENSION_ORIGIN = 'chrome-extension://jamiahhkhimbjajeddmncinemlmfkfhn';

interface BridgeConfig {
  port: number;
  token: string;
  allowedOrigins: string[];
  /** 提示注入閘門只有 codex 實測通過;其餘預設關閉 */
  providers: { claude: boolean; codex: boolean; gemini: boolean };
  openai?: OpenAIProviderOptions[];
}

function configPath(): string {
  const base = process.env.APPDATA ?? path.join(os.homedir(), '.config');
  return path.join(base, 'agent-translator', 'config.json');
}

function loadConfig(): { cfg: BridgeConfig; created: boolean } {
  const file = configPath();
  if (existsSync(file)) return { cfg: JSON.parse(readFileSync(file, 'utf8')) as BridgeConfig, created: false };
  const cfg: BridgeConfig = {
    port: 47821,
    token: randomBytes(24).toString('base64url'),
    allowedOrigins: [EXTENSION_ORIGIN],
    providers: { claude: false, codex: true, gemini: false },
    openai: [],
  };
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(cfg, null, 2) + '\n', 'utf8');
  return { cfg, created: true };
}

async function main(): Promise<void> {
  const { cfg, created } = loadConfig();
  const unverified = '提示注入閘門未驗證,屬高風險';
  const providers: Provider[] = [];
  if (cfg.providers.codex) providers.push(createCliProvider({ kind: 'codex' }));
  if (cfg.providers.claude) providers.push(createCliProvider({ kind: 'claude', note: unverified }));
  if (cfg.providers.gemini) providers.push(createCliProvider({ kind: 'gemini', note: unverified }));
  for (const o of cfg.openai ?? []) providers.push(createOpenAIProvider(o));

  const handle = await createBridgeServer({
    host: '127.0.0.1',
    port: cfg.port,
    token: cfg.token,
    allowedOrigins: cfg.allowedOrigins,
    providers,
  });
  console.log(`[agent-translator bridge] MCP 端點:${handle.url}`);
  console.log(`[agent-translator bridge] 設定檔:${configPath()}`);
  console.log(`[agent-translator bridge] 已啟用後端:${providers.map((p) => p.id).join(', ') || '(無)'}`);
  if (created || process.argv.includes('--show-token')) {
    console.log(`[agent-translator bridge] token(貼到擴充的設定頁):${cfg.token}`);
  }
  const stop = () => void handle.close().then(() => process.exit(0));
  process.on('SIGINT', stop);
  process.on('SIGTERM', stop);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
