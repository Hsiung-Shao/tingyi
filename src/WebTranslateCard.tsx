import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ModelInfo, WebStatus } from "./lib/types";

const STATUS_TEXT: Record<WebStatus["status"], string> = {
  "not-downloaded": "翻譯模型尚未下載",
  downloading: "下載中",
  stopped: "未啟動",
  starting: "啟動中…",
  ready: "執行中",
  error: "發生錯誤",
};

const DOT: Record<WebStatus["status"], string> = {
  "not-downloaded": "bg-neutral-500",
  downloading: "bg-sky-400",
  stopped: "bg-neutral-500",
  starting: "bg-amber-400",
  ready: "bg-emerald-400",
  error: "bg-rose-500",
};

/** 網頁翻譯(Chrome 擴充)的狀態、啟動/停止與模型選擇。模型和字幕分開設定,共用同一台伺服器。 */
export default function WebTranslateCard({ models }: { models: ModelInfo[] }) {
  const [st, setSt] = useState<WebStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 讀不到只是暫時不更新顯示,下一次事件會再讀,故不報錯。
  const refresh = useCallback(() => invoke<WebStatus>("web_status").then(setSt).catch(() => {}), []);

  useEffect(() => {
    refresh();
    // runtime 改變(啟動、就緒、錯誤、換模型)與字幕開始/停止時重讀
    const a = listen("web-status", () => refresh());
    const b = listen("session-running", () => refresh());
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, [refresh]);

  const run = async (command: string, args?: Record<string, unknown>) => {
    setError(null);
    try {
      await invoke(command, args);
    } catch (e) {
      setError(String(e));
    } finally {
      refresh();
    }
  };

  if (!st) return null;
  const active = st.status === "ready" || st.status === "starting";
  const borrowed = st.status === "ready" && st.subtitles && st.server_model !== null && st.server_model !== st.web_model;

  return (
    <section className="flex flex-col gap-2" data-role="web-translate">
      <label className="text-neutral-400">網頁翻譯(Chrome 擴充)</label>
      <div className="flex items-center gap-2">
        <span className={`h-2 w-2 rounded-full ${DOT[st.status]}`} />
        <span className="flex-1 text-neutral-200" data-role="web-status">
          {STATUS_TEXT[st.status]}
          {st.status === "ready" && st.server_model && <span className="text-neutral-500"> · {st.server_model}</span>}
        </span>
        <button
          className={`rounded px-2 py-0.5 text-xs disabled:opacity-50 ${active ? "bg-neutral-800 hover:bg-neutral-700" : "bg-sky-800 hover:bg-sky-700"}`}
          onClick={() => run(active ? "web_stop" : "web_start")}
          disabled={st.status === "not-downloaded" || st.status === "downloading"}
          data-role="web-toggle"
        >
          {/* 與主視窗字幕的「開始/停止」分開命名,一眼看出按的是哪一個 */}
          {active ? "停止網頁翻譯" : "啟動網頁翻譯"}
        </button>
      </div>
      {borrowed && (
        <div className="text-xs text-amber-400">
          暫用字幕的 {st.server_model},字幕停止後切回 {st.web_model}
        </div>
      )}
      {st.message && <div className={`text-xs ${st.status === "ready" ? "text-amber-400" : "text-rose-400"}`}>{st.message}</div>}
      <select
        className="bg-neutral-900 border border-neutral-700 rounded px-2 py-1"
        aria-label="網頁翻譯模型"
        value={st.web_model}
        onChange={(e) => run("set_web_model", { model: e.target.value })}
      >
        {models.map((m) => (
          <option key={m.name} value={m.name} disabled={!m.present}>
            {m.name} {m.present ? "" : "(未下載)"}
          </option>
        ))}
      </select>
      {error && <div className="text-xs text-rose-400">{error}</div>}
      <p className="text-neutral-500 text-xs">
        Chrome 擴充第一次翻譯時也會自動啟動;各網站要不要自動翻譯、YouTube 字幕要不要翻,在擴充的 popup 設定。
      </p>
    </section>
  );
}
