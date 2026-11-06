import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ModelDownloadProgress, RoutedModelState } from "./lib/types";

const mb = (bytes: number) => `${Math.round(bytes / 1_000_000)} MB`;

/** 各組下載卡的狀態指令、標題與「改用腳本下載」的提示。 */
const GROUPS = {
  routed: {
    command: "routed_models_status",
    title: "專用模型(CPU,選「專用模型」引擎時使用)",
    scriptFlag: (id: string) => (id === "reazonspeech" ? " -WithReazonSpeech" : " -WithParakeet"),
  },
  base: {
    command: "base_packages_status",
    title: "辨識基本模型(第一次使用前必須下載;說話者辨識選用)",
    scriptFlag: () => "",
  },
  engine: {
    command: "engine_packages_status",
    title: "所選辨識引擎的模型",
    scriptFlag: () => "",
  },
  translate: {
    command: "translate_packages_status",
    title: "翻譯模型(本機翻譯與瀏覽器網頁翻譯共用)",
    scriptFlag: () => "",
  },
} as const;

/** 可下載模型的下載卡(辨識基本模型、專用辨識模型,或翻譯用的 llama.cpp 與 Hy-MT2):狀態、下載、進度、取消、繼續。
 *  進度只放在這個元件內,避免每則事件讓整個主視窗重繪。 */
export default function RoutedModels({
  group = "routed",
  engine,
  running,
  dataDir,
  onChanged,
  title,
  activeKey,
  onUse,
}: {
  group?: keyof typeof GROUPS;
  /** group="engine" 時:下拉選單所選的引擎 id。 */
  engine?: string;
  running: boolean;
  dataDir?: string;
  onChanged: () => void;
  /** 蓋掉預設標題(設定頁每個引擎一區時用) */
  title?: string;
  /** 設定頁:目前選用的翻譯模型名稱;有 onUse 時已下載的翻譯模型顯示「使用這個」 */
  activeKey?: string;
  onUse?: (key: string) => void;
}) {
  const g = GROUPS[group];
  const [models, setModels] = useState<RoutedModelState[]>([]);
  const [progress, setProgress] = useState<Record<string, ModelDownloadProgress>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [finishedWhileRunning, setFinishedWhileRunning] = useState(false);
  // 事件監聽只訂閱一次,用 ref 讀目前是否執行中與這組有哪些模型
  const runningRef = useRef(running);
  const modelIds = useRef(new Set<string>());
  modelIds.current = new Set(models.map((m) => m.id));

  const refresh = useCallback(
    () =>
      invoke<RoutedModelState[]>(g.command, engine === undefined ? undefined : { engine })
        .then(setModels)
        .catch(() => {}),
    [g.command, engine],
  );

  useEffect(() => {
    refresh();
    const un = listen<ModelDownloadProgress>("model-download", (e) => {
      const p = e.payload;
      // 兩張卡共用同一個事件,只處理自己這組的模型
      if (!modelIds.current.has(p.id)) return;
      setProgress((prev) => ({ ...prev, [p.id]: p }));
      if (p.phase === "error") setErrors((prev) => ({ ...prev, [p.id]: p.message ?? "下載失敗" }));
      if (p.phase === "done" || p.phase === "cancelled" || p.phase === "error") {
        refresh();
        if (p.phase === "done") {
          onChanged();
          // 只有執行中才完成的下載要等下次開始;開始前就下完的已經載入了
          if (runningRef.current) setFinishedWhileRunning(true);
        }
      }
    });
    return () => {
      un.then((f) => f());
    };
    // onChanged 只用來通知上層重查,不需要重新訂閱
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  useEffect(() => {
    runningRef.current = running;
    if (!running) setFinishedWhileRunning(false);
  }, [running]);

  const start = async (id: string) => {
    setErrors((prev) => ({ ...prev, [id]: "" }));
    try {
      await invoke("download_model", { id });
      await refresh();
    } catch (e) {
      setErrors((prev) => ({ ...prev, [id]: String(e) }));
    }
  };

  const cancel = async (id: string) => {
    // 只會在下載剛好結束時回 false,結果由 model-download 事件通知,不需處理
    await invoke("cancel_model_download", { id }).catch(() => {});
  };

  // SenseVoice 等不需另外下載的引擎:不顯示卡片
  if (models.length === 0) return null;

  return (
    <div data-group={group} className="flex flex-col gap-2 rounded border border-neutral-800 bg-neutral-900/50 px-2 py-2 text-xs">
      <div className="text-neutral-400">{title ?? g.title}</div>
      {models.map((m) => {
        const p = progress[m.id];
        const active = m.downloading || p?.phase === "downloading" || p?.phase === "verifying";
        const done = active ? (p?.done ?? m.done) : m.done;
        const pct = m.size_bytes > 0 ? Math.min(100, Math.floor((done / m.size_bytes) * 100)) : 0;
        return (
          <div key={m.id} className="flex flex-col gap-1" data-model-id={m.id}>
            <div className="flex items-center gap-2">
              <span className="flex-1 text-neutral-200">
                {m.label}
                <span className="text-neutral-500"> {mb(m.size_bytes)}</span>
              </span>
              {m.gguf && onUse && m.ready && (
                m.gguf === activeKey ? (
                  <span className="text-sky-300" data-role="model-active">
                    使用中
                  </span>
                ) : (
                  <button
                    className="rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700 disabled:opacity-40"
                    data-role="model-use"
                    disabled={running}
                    title={running ? "停止後才能換模型" : undefined}
                    onClick={() => onUse(m.gguf!)}
                  >
                    使用這個
                  </button>
                )
              )}
              {m.ready ? (
                <span className="text-emerald-400" data-role="model-ready">
                  已下載
                </span>
              ) : active ? (
                <button className="rounded bg-neutral-800 px-2 py-0.5 hover:bg-neutral-700" onClick={() => cancel(m.id)}>
                  取消
                </button>
              ) : (
                <button className="rounded bg-sky-800 px-2 py-0.5 hover:bg-sky-700" onClick={() => start(m.id)}>
                  {done > 0 ? `繼續(${pct}%)` : "下載"}
                </button>
              )}
            </div>
            {active && (
              <div className="flex items-center gap-2">
                <div className="h-1.5 flex-1 rounded bg-neutral-800">
                  <div className="h-1.5 rounded bg-sky-500" style={{ width: `${pct}%` }} />
                </div>
                <span className="w-24 text-right text-neutral-400" data-role="model-progress">
                  {p?.phase === "verifying" ? "校驗中…" : `${pct}%${p?.bps ? ` · ${(p.bps / 1_000_000).toFixed(1)} MB/s` : ""}`}
                </span>
              </div>
            )}
            {errors[m.id] && (
              <div className="text-rose-400" data-role="model-error">
                {errors[m.id]}
                {/* 下載腳本不含這些辨識引擎(whisper-server、Qwen3、Cohere) */}
                {group !== "engine" && (
                  <div className="text-neutral-500">
                    也可以用 repo 的 scripts/download-models.ps1{g.scriptFlag(m.id)} 下載到
                    {dataDir ? ` ${dataDir}\\models` : " 資料目錄的 models"}。
                  </div>
                )}
              </div>
            )}
          </div>
        );
      })}
      {finishedWhileRunning && <div className="text-amber-400">下載完成,下次按「開始」時生效。</div>}
      <div className="text-neutral-500">
        授權:
        {models.map((m, i) => (
          <span key={m.id}>
            {i > 0 && "、"}
            <button className="underline hover:text-neutral-300" onClick={() => openUrl(m.license_url).catch(() => {})}>
              {m.label}
            </button>
            ({m.license})
          </span>
        ))}
      </div>
    </div>
  );
}
