import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ModelDownloadProgress, RoutedModelState } from "./lib/types";

/** 各組套件的狀態指令。 */
export type PackageCommand = "routed_models_status" | "base_packages_status" | "engine_packages_status" | "translate_packages_status";

export const mb = (bytes: number) => (bytes >= 1_000_000_000 ? `${(bytes / 1_000_000_000).toFixed(1)} GB` : `${Math.round(bytes / 1_000_000)} MB`);

/** 一個套件目前要怎麼顯示(合併後端狀態與即時進度事件)。 */
export interface PackageView {
  m: RoutedModelState;
  active: boolean;
  pct: number;
  /** 進度文字:「42% · 38.5 MB/s」或「校驗中…」 */
  progressText: string;
  error: string;
  start: () => void;
  cancel: () => void;
}

/** 一組套件的狀態、下載、取消、進度。進度只放在呼叫端元件內,避免每則事件讓整個主視窗重繪。 */
export function usePackages(command: PackageCommand, engine: string | undefined, running: boolean, onChanged: () => void) {
  const [models, setModels] = useState<RoutedModelState[]>([]);
  const [progress, setProgress] = useState<Record<string, ModelDownloadProgress>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [finishedWhileRunning, setFinishedWhileRunning] = useState(false);
  // 事件監聽只訂閱一次,用 ref 讀目前是否執行中與這組有哪些模型
  const runningRef = useRef(running);
  const modelIds = useRef(new Set<string>());
  modelIds.current = new Set(models.map((m) => m.id));
  const onChangedRef = useRef(onChanged);
  onChangedRef.current = onChanged;

  const refresh = useCallback(
    () =>
      invoke<RoutedModelState[]>(command, engine === undefined ? undefined : { engine })
        .then(setModels)
        .catch(() => {}),
    [command, engine],
  );

  useEffect(() => {
    refresh();
    const un = listen<ModelDownloadProgress>("model-download", (e) => {
      const p = e.payload;
      // 各組共用同一個事件,只處理自己這組的模型
      if (!modelIds.current.has(p.id)) return;
      setProgress((prev) => ({ ...prev, [p.id]: p }));
      if (p.phase === "error") setErrors((prev) => ({ ...prev, [p.id]: p.message ?? "下載失敗" }));
      if (p.phase === "done" || p.phase === "cancelled" || p.phase === "error") {
        refresh();
        if (p.phase === "done") {
          onChangedRef.current();
          // 只有執行中才完成的下載要等下次開始;開始前就下完的已經載入了
          if (runningRef.current) setFinishedWhileRunning(true);
        }
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  useEffect(() => {
    runningRef.current = running;
    if (!running) setFinishedWhileRunning(false);
  }, [running]);

  const views: PackageView[] = models.map((m) => {
    const p = progress[m.id];
    const active = m.downloading || p?.phase === "downloading" || p?.phase === "verifying";
    const done = active ? (p?.done ?? m.done) : m.done;
    const pct = m.size_bytes > 0 ? Math.min(100, Math.floor((done / m.size_bytes) * 100)) : 0;
    return {
      m,
      active,
      pct: m.ready ? 100 : done > 0 || active ? pct : 0,
      progressText: p?.phase === "verifying" ? "校驗中…" : `${pct}%${p?.bps ? ` · ${(p.bps / 1_000_000).toFixed(1)} MB/s` : ""}`,
      error: errors[m.id] ?? "",
      start: async () => {
        setErrors((prev) => ({ ...prev, [m.id]: "" }));
        try {
          await invoke("download_model", { id: m.id });
          await refresh();
        } catch (e) {
          setErrors((prev) => ({ ...prev, [m.id]: String(e) }));
        }
      },
      // 只會在下載剛好結束時回 false,結果由 model-download 事件通知,不需處理
      cancel: () => void invoke("cancel_model_download", { id: m.id }).catch(() => {}),
    };
  });
  return { views, finishedWhileRunning };
}

/** 狀態徽章 */
export function Badge({ tone, children, role }: { tone: "active" | "ready" | "muted" | "warn"; children: React.ReactNode; role?: string }) {
  const cls = {
    active: "bg-sky-950 text-sky-200",
    ready: "bg-emerald-950 text-emerald-300",
    muted: "bg-neutral-800 text-neutral-400",
    warn: "bg-amber-950 text-amber-300",
  }[tone];
  return (
    <span className={`shrink-0 whitespace-nowrap rounded-full px-2.5 py-0.5 text-xs ${cls}`} data-role={role}>
      {children}
    </span>
  );
}

const btnPrimary = "min-h-10 rounded-lg bg-sky-700 px-4 text-white hover:bg-sky-600 disabled:opacity-40";
const btnSecondary = "min-h-10 rounded-lg border border-neutral-700 bg-neutral-800 px-4 hover:bg-neutral-700 disabled:opacity-40";

/** 下載中:進度條 + 取消;否則下載/繼續鈕。已下載時不顯示。 */
function DownloadControl({ v, compact }: { v: PackageView; compact?: boolean }) {
  if (v.m.ready) return null;
  if (v.active) {
    return (
      <div className="flex flex-col gap-1.5">
        <div className="h-1.5 rounded bg-neutral-800">
          <div className="h-1.5 rounded bg-sky-400" style={{ width: `${v.pct}%` }} />
        </div>
        <div className="flex justify-between text-xs text-neutral-400">
          <span data-role="model-progress">{v.progressText}</span>
          <button className="hover:text-neutral-200" onClick={v.cancel}>
            取消
          </button>
        </div>
      </div>
    );
  }
  const label = v.m.done > 0 ? `繼續(${v.pct}%)` : compact ? "下載" : `下載 ${mb(v.m.size_bytes)}`;
  return (
    <button className={compact ? "min-h-8 rounded-md bg-sky-700 px-3 text-xs text-white hover:bg-sky-600" : btnPrimary} onClick={v.start}>
      {label}
    </button>
  );
}

function ErrorLine({ v, scriptHint }: { v: PackageView; scriptHint?: string }) {
  if (!v.error) return null;
  return (
    <div className="text-xs text-rose-400" data-role="model-error">
      {v.error}
      {scriptHint !== undefined && <div className="text-neutral-500">也可以用 repo 的 scripts/download-models.ps1{scriptHint} 下載。</div>}
    </div>
  );
}

function License({ m }: { m: RoutedModelState }) {
  return (
    <button
      className="rounded-md bg-neutral-800/80 px-2 py-0.5 text-neutral-400 hover:text-neutral-200"
      title="開啟授權頁面"
      onClick={() => openUrl(m.license_url).catch(() => {})}
    >
      {m.license}
    </button>
  );
}

/** 翻譯模型卡片:名稱、用途、大小與授權、狀態、使用/下載。 */
export function ModelCard({
  v,
  note,
  isActive,
  running,
  onUse,
}: {
  v: PackageView;
  note?: string;
  isActive: boolean;
  running: boolean;
  onUse?: () => void;
}) {
  const { m } = v;
  const badge = isActive && m.ready ? (
    <Badge tone="active" role="model-active">
      使用中
    </Badge>
  ) : m.ready ? (
    <Badge tone="ready" role="model-ready">
      已下載
    </Badge>
  ) : v.active ? (
    <Badge tone="muted">下載中</Badge>
  ) : (
    <Badge tone="muted">未下載</Badge>
  );
  return (
    <article
      data-model-id={m.id}
      className={`flex flex-col gap-2.5 rounded-xl border p-4 ${isActive && m.ready ? "border-sky-400 bg-sky-950/20" : "border-neutral-800 bg-neutral-900/60"}`}
    >
      <div className="flex items-center gap-2">
        <span className="font-semibold">{m.label}</span>
        <span className="ml-auto">{badge}</span>
      </div>
      {note && <p className="text-[13px] leading-relaxed text-neutral-400">{note}</p>}
      <div className="flex flex-wrap gap-1.5 text-xs text-neutral-400">
        <span className="rounded-md bg-neutral-800/80 px-2 py-0.5">{mb(m.size_bytes)}</span>
        <License m={m} />
      </div>
      <div className="mt-auto">
        {m.ready && onUse && !isActive && (
          <button
            className={`${btnSecondary} w-full`}
            data-role="model-use"
            disabled={running}
            title={running ? "停止後才能換模型" : undefined}
            onClick={onUse}
          >
            使用這個
          </button>
        )}
        {!m.ready && <DownloadControl v={v} />}
      </div>
      <ErrorLine v={v} scriptHint="" />
    </article>
  );
}

/** 卡片內的一列(引擎需要的檔案、基本模型):名稱+大小、已下載或下載鈕。 */
export function PackageRow({ v, sub, scriptHint }: { v: PackageView; sub?: string; scriptHint?: string }) {
  const { m } = v;
  return (
    <div data-model-id={m.id} className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <span className="flex min-w-0 flex-1 flex-col">
          <span>{m.label}</span>
          <span className="text-xs text-neutral-500">{sub ?? mb(m.size_bytes)}</span>
        </span>
        {m.ready && (
          <span className="text-xs text-emerald-300" data-role="model-ready">
            已下載
          </span>
        )}
        {!m.ready && !v.active && <DownloadControl v={v} compact />}
      </div>
      {v.active && <DownloadControl v={v} compact />}
      <ErrorLine v={v} scriptHint={scriptHint} />
    </div>
  );
}
