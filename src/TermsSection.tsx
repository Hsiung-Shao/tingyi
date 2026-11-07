import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Badge } from "./Packages";
import type { TermEntry, TermPackInfo } from "./lib/types";

/** 設定頁「遊戲術語」:匯入、已安裝清單(使用 / 移除)、查詢內容、開啟資料夾。 */
export default function TermsSection({
  packs,
  active,
  running,
  onUse,
  onChanged,
}: {
  packs: TermPackInfo[];
  active: string;
  running: boolean;
  onUse: (id: string) => void;
  /** 匯入或移除後重新讀清單 */
  onChanged: () => void;
}) {
  const [msg, setMsg] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [queryPack, setQueryPack] = useState("");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<TermEntry[]>([]);

  // 查詢目標:沒選過就用使用中的,再不然第一個
  const target = packs.some((p) => p.id === queryPack) ? queryPack : packs.some((p) => p.id === active) ? active : (packs[0]?.id ?? "");

  useEffect(() => {
    const q = query.trim();
    if (!target || !q) {
      setResults([]);
      return;
    }
    const t = setTimeout(() => {
      invoke<TermEntry[]>("search_term_pack", { id: target, query: q })
        .then(setResults)
        .catch((e) => setMsg({ tone: "err", text: String(e) }));
    }, 200);
    return () => clearTimeout(t);
  }, [target, query]);

  const doImport = async (path: string, replace: boolean): Promise<void> => {
    try {
      const info = await invoke<TermPackInfo>("import_term_pack", { path, replace });
      setMsg({ tone: "ok", text: `已匯入 ${info.label}(${info.entries.toLocaleString()} 詞)` });
      onChanged();
    } catch (e) {
      const s = String(e);
      if (s.startsWith("exists:")) {
        const id = s.slice("exists:".length);
        if (window.confirm(`已經有「${id}」術語包,要用這份取代嗎?`)) return doImport(path, true);
        setMsg(null);
        return;
      }
      setMsg({ tone: "err", text: `匯入失敗:${s}` });
    }
  };

  const pick = async () => {
    setMsg(null);
    const path = await open({ multiple: false, directory: false, filters: [{ name: "術語包", extensions: ["json"] }] });
    if (typeof path === "string") await doImport(path, false);
  };

  const remove = async (id: string) => {
    setConfirmRemove(null);
    try {
      await invoke("remove_term_pack", { id });
      setMsg({ tone: "ok", text: `已移除「${id}」` });
      onChanged();
    } catch (e) {
      setMsg({ tone: "err", text: `移除失敗:${String(e)}` });
    }
  };

  return (
    <section data-section="terms" className="flex max-w-[900px] flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <h1 className="text-[22px] font-bold">遊戲術語</h1>
        <p className="text-neutral-400">
          匯入術語包後,字幕頁「語言」卡可以選用:專有名詞譯成官方名、修正常見誤聽,用 Whisper 時再加熱詞提示。術語包只存在這台電腦。
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <button className="min-h-10 rounded-lg bg-sky-700 px-4 font-medium text-white hover:bg-sky-600" data-role="term-import" onClick={() => void pick()}>
          匯入術語包…
        </button>
        <button
          className="min-h-10 rounded-lg border border-neutral-700 bg-neutral-800 px-4 hover:bg-neutral-700"
          onClick={() => invoke("open_term_packs_dir").catch((e) => setMsg({ tone: "err", text: String(e) }))}
        >
          開啟術語包資料夾
        </button>
        {msg && (
          <span className={`text-sm ${msg.tone === "ok" ? "text-emerald-300" : "text-rose-400"}`} data-role="term-msg">
            {msg.text}
          </span>
        )}
      </div>

      {packs.length === 0 ? (
        <div className="rounded-xl border border-dashed border-neutral-700 px-5 py-6 text-neutral-400" data-role="term-empty">
          尚未安裝術語包。術語包是 JSON 檔,用 repo 的 <code className="text-neutral-300">scripts/poe-terms/build_term_pack.py</code> 從本機 POE 資料庫產生,再按「匯入術語包…」選取。
        </div>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-3">
          {packs.map((p) => (
            <article
              key={p.id}
              data-pack-id={p.id}
              className={`flex flex-col gap-2.5 rounded-xl border p-4 ${p.id === active ? "border-sky-400 bg-sky-950/20" : "border-neutral-800 bg-neutral-900/60"}`}
            >
              <div className="flex items-center gap-2">
                <span className="font-semibold">{p.label}</span>
                <span className="text-xs text-neutral-500">{p.game_version}</span>
                <span className="ml-auto">{p.id === active && <Badge tone="active" role="term-active">使用中</Badge>}</span>
              </div>
              <div className="flex flex-wrap gap-1.5 text-xs text-neutral-400">
                <span className="rounded-md bg-neutral-800/80 px-2 py-0.5">{p.entries.toLocaleString()} 詞</span>
                <span className="rounded-md bg-neutral-800/80 px-2 py-0.5">誤聽修正 {p.aliases}</span>
                <span className="rounded-md bg-neutral-800/80 px-2 py-0.5">熱詞 {p.hotwords}</span>
                {p.generated_at && <span className="rounded-md bg-neutral-800/80 px-2 py-0.5">產生於 {p.generated_at.slice(0, 10)}</span>}
              </div>
              <div className="mt-auto flex gap-2">
                {p.id !== active && (
                  <button
                    className="min-h-9 flex-1 rounded-lg border border-neutral-700 bg-neutral-800 px-3 hover:bg-neutral-700 disabled:opacity-40"
                    data-role="term-use"
                    disabled={running}
                    onClick={() => onUse(p.id)}
                  >
                    使用這個
                  </button>
                )}
                {confirmRemove === p.id ? (
                  <>
                    <button className="min-h-9 rounded-lg bg-rose-700 px-3 text-white hover:bg-rose-600" data-role="term-remove-confirm" onClick={() => void remove(p.id)}>
                      確定移除
                    </button>
                    <button className="min-h-9 rounded-lg px-3 text-neutral-400 hover:text-neutral-200" onClick={() => setConfirmRemove(null)}>
                      取消
                    </button>
                  </>
                ) : (
                  <button
                    className="min-h-9 rounded-lg px-3 text-neutral-400 hover:text-rose-300 disabled:opacity-40"
                    data-role="term-remove"
                    disabled={running && p.id === active}
                    onClick={() => setConfirmRemove(p.id)}
                  >
                    移除
                  </button>
                )}
              </div>
            </article>
          ))}
        </div>
      )}

      {packs.length > 0 && (
        <div className="flex flex-col gap-3 rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
          <h2 className="text-[15px] font-bold">查詢術語</h2>
          <div className="flex flex-wrap gap-2">
            <select
              className="min-h-10 rounded-lg border border-neutral-700 bg-neutral-950 px-2"
              value={target}
              onChange={(e) => setQueryPack(e.target.value)}
              aria-label="查詢哪個術語包"
            >
              {packs.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
            <input
              className="min-h-10 min-w-0 flex-1 rounded-lg border border-neutral-700 bg-neutral-950 px-3"
              placeholder="輸入英文或中文,例如 Chaos Orb、奇塔"
              aria-label="查詢術語"
              data-role="term-query"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          {query.trim() && (
            <div className="flex flex-col divide-y divide-neutral-800 text-[13px]" data-role="term-results">
              {results.length === 0 ? (
                <span className="py-2 text-neutral-500">沒有符合的術語</span>
              ) : (
                results.map((r) => (
                  <div key={r.en} className="flex items-baseline gap-3 py-1.5" data-term-en={r.en}>
                    <span className="w-2/5 min-w-0 truncate text-neutral-200">{r.en}</span>
                    <span className="flex-1 font-medium text-white">{r.zh}</span>
                    {r.alt.length > 0 && <span className="text-xs text-neutral-500">另見:{r.alt.slice(0, 2).join("、")}</span>}
                    <span className="text-xs text-neutral-600">{r.src}</span>
                  </div>
                ))
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
