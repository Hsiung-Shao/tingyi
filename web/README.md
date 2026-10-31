# Agent Translator

類似「沉浸式翻譯」的 Chrome 擴充:在網頁原文段落下方插入譯文(雙語對照)。翻譯後端用你自己的訂閱 agent 或本地模型。

```
[擴充 content script] → [擴充 background] ─MCP(Streamable HTTP)→ [本機 bridge] → codex / claude / gemini CLI
                                          └─ 直連 OpenAI 相容端點(Ollama、LM Studio、llama-server)
```

## 安裝

需要 Node 24 與 pnpm 10。

```bash
pnpm install
pnpm build
```

1. 打開 Chrome 的 `chrome://extensions`,開啟「開發人員模式」,按「載入未封裝項目」,選 `extension/dist`。擴充 ID 固定為 `jamiahhkhimbjajeddmncinemlmfkfhn`。
2. 用訂閱 agent 的話,啟動 bridge(第一次會產生 token 並印出來):

   ```bash
   pnpm bridge
   ```

3. 打開擴充的「進階設定」,把 token 貼到 bridge 的 token 欄位,然後按「測試翻譯」。
4. 到任一網頁按 `Alt+A`,或在 popup 按「翻譯此頁」。再按一次就回到原文。

## 後端

| 後端 | 怎麼開 | 狀態 |
|---|---|---|
| Codex CLI(ChatGPT 訂閱) | 預設啟用 | 提示注入閘門已驗證 |
| Claude Code CLI | 先執行 `claude auth login`,再把 bridge 設定檔裡的 `providers.claude` 改成 `true` | 閘門未驗證,屬高風險 |
| Gemini CLI | 需要能登入的帳號(Workspace 帳號要設 `GOOGLE_CLOUD_PROJECT`),再把 `providers.gemini` 改成 `true` | 閘門未驗證,屬高風險 |
| 本地模型 | popup 選「直連」,在設定頁填 Base URL 與模型。Hy-MT2 這類翻譯專用模型請選「一次一段純文字」 | 已用 llama-server + Hy-MT2 1.8B 實測 |

bridge 設定檔:`%APPDATA%\agent-translator\config.json`(含 token,不要分享)。

## 安全設計

- CLI agent 一律關閉所有工具,在空的暫存目錄執行,網頁原文只經 stdin 傳入。
- bridge 只聽 `127.0.0.1`。請求必須帶 token;`Origin` 不在白名單回 403;`Host` 不是 localhost 也回 403,用來防 DNS rebinding。

## 開發

```bash
pnpm test        # vitest(core / bridge / extension)
pnpm typecheck   # tsc
node scripts/e2e.mjs --out <證據目錄> --route direct --base-url http://127.0.0.1:8178/v1 --format single
```

`scripts/e2e.mjs` 會用 ms-playwright 的 Chromium 真的載入擴充、翻譯本機 fixture 頁並截圖。
