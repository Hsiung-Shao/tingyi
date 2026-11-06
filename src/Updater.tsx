import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Badge } from "./Packages";

export interface UpdateInfo {
  version: string;
  current: string;
  notes: string;
  date: string | null;
  /** 該不該主動提示(執行中或略過這版時 false) */
  notify: boolean;
}

type Phase = "idle" | "checking" | "none" | "found" | "downloading" | "verified" | "installing" | "error";

const DISMISS_KEY = "tingyi.update.dismissed";
const readDismissed = () => {
  try {
    return localStorage.getItem(DISMISS_KEY);
  } catch {
    return null;
  }
};

/** 自動更新的狀態:開啟 5 秒後自動檢查一次(失敗不打擾),設定頁可手動檢查、下載並更新、略過這版。 */
export function useUpdater() {
  const [current, setCurrent] = useState("");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState("");
  const [progress, setProgress] = useState<{ done: number; total: number | null }>({ done: 0, total: null });

  const check = useCallback(async (manual: boolean) => {
    setPhase("checking");
    setError("");
    try {
      const r = await invoke<UpdateInfo | null>("check_update", { dismissed: manual ? null : readDismissed() });
      setInfo(r);
      setPhase(r ? "found" : "none");
    } catch (e) {
      // 自動檢查失敗(沒網路等)不顯示;手動檢查才說
      setPhase(manual ? "error" : "idle");
      if (manual) setError(String(e));
    }
  }, []);

  useEffect(() => {
    invoke<string>("app_version").then(setCurrent).catch(() => {});
    const t = setTimeout(() => void check(false), 5000);
    const un = listen<{ done: number; total: number | null }>("update-progress", (e) => setProgress(e.payload));
    return () => {
      clearTimeout(t);
      un.then((f) => f());
    };
  }, [check]);

  const install = async () => {
    setPhase("downloading");
    setError("");
    setProgress({ done: 0, total: null });
    try {
      const r = await invoke<string>("install_update");
      setPhase(r === "dry-run-verified" ? "verified" : "installing");
    } catch (e) {
      setPhase("error");
      setError(String(e));
    }
  };

  const dismiss = () => {
    if (!info) return;
    try {
      localStorage.setItem(DISMISS_KEY, info.version);
    } catch {
      // 存不了只是下次還會提示
    }
    setInfo({ ...info, notify: false });
  };

  return { current, info, phase, error, progress, check, install, dismiss };
}

export type Updater = ReturnType<typeof useUpdater>;

/** 標題列提示:有新版且該提示時顯示按鈕;執行中只亮小點,不打擾。 */
export function UpdateHint({ u, running, onOpen }: { u: Updater; running: boolean; onOpen: () => void }) {
  if (!u.info || u.phase === "none") return null;
  if (running || !u.info.notify) {
    return running ? (
      <button className="h-2 w-2 rounded-full bg-sky-400" title={`有新版 ${u.info.version},停止後可到設定更新`} aria-label="有新版" onClick={onOpen} />
    ) : null;
  }
  return (
    <button className="rounded-full bg-sky-900 px-3 py-1 text-xs text-sky-100 hover:bg-sky-800" data-role="update-hint" onClick={onOpen}>
      有新版 {u.info.version}
    </button>
  );
}

/** 設定頁「關於與更新」。 */
export function AboutSection({ u, running }: { u: Updater; running: boolean }) {
  const pct = u.progress.total ? Math.min(100, Math.floor((u.progress.done / u.progress.total) * 100)) : null;
  const busy = u.phase === "checking" || u.phase === "downloading" || u.phase === "installing";
  return (
    <section data-section="about" className="flex max-w-[760px] flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <h1 className="text-[22px] font-bold">關於與更新</h1>
        <p className="text-neutral-400">每次開啟聽譯會自動檢查一次;也可以在這裡手動檢查。</p>
      </div>
      <div className="flex items-center gap-4 rounded-xl border border-neutral-800 bg-neutral-900/60 px-5 py-4">
        <div className="flex flex-1 flex-col gap-0.5">
          <span className="font-medium">聽譯 {u.current || "—"}</span>
          <span className="text-xs text-neutral-500" data-role="update-status">
            {u.phase === "checking"
              ? "檢查中…"
              : u.phase === "none"
                ? "已是最新版"
                : u.info
                  ? `有新版 ${u.info.version}`
                  : "尚未檢查"}
          </span>
        </div>
        <button
          className="min-h-10 rounded-lg border border-neutral-700 bg-neutral-800 px-4 hover:bg-neutral-700 disabled:opacity-40"
          data-role="update-check"
          disabled={busy}
          onClick={() => void u.check(true)}
        >
          檢查更新
        </button>
      </div>
      {u.info && (
        <article className="flex flex-col gap-3 rounded-xl border border-sky-400/60 bg-sky-950/20 p-5" data-role="update-card">
          <div className="flex items-center gap-2">
            <span className="text-lg font-semibold">新版 {u.info.version}</span>
            {u.info.date && <span className="text-xs text-neutral-500">{u.info.date.slice(0, 10)}</span>}
            <span className="ml-auto">
              <Badge tone="active">目前 {u.info.current}</Badge>
            </span>
          </div>
          {u.info.notes && (
            <div className="max-h-60 overflow-y-auto whitespace-pre-wrap rounded-lg bg-neutral-900/70 px-4 py-3 text-[13px] leading-relaxed text-neutral-300" data-role="update-notes">
              {u.info.notes}
            </div>
          )}
          {u.phase === "downloading" && (
            <div className="flex flex-col gap-1.5">
              <div className="h-1.5 rounded bg-neutral-800">
                <div className="h-1.5 rounded bg-sky-400" style={{ width: `${pct ?? 5}%` }} />
              </div>
              <span className="text-xs text-neutral-400" data-role="update-progress">
                下載中 {pct != null ? `${pct}%` : `${Math.round(u.progress.done / 1_000_000)} MB`}
              </span>
            </div>
          )}
          {u.phase === "verified" && (
            <p className="text-sm text-emerald-300" data-role="update-verified">
              下載完成,簽章驗證通過(測試模式,沒有安裝)。
            </p>
          )}
          {u.phase === "installing" && <p className="text-sm text-sky-200">安裝中,聽譯會自動關閉並重新開啟…</p>}
          <div className="flex flex-wrap items-center gap-2">
            <button
              className="min-h-10 rounded-lg bg-sky-700 px-5 font-medium text-white hover:bg-sky-600 disabled:opacity-40"
              data-role="update-install"
              disabled={busy || u.phase === "verified"}
              onClick={() => void u.install()}
            >
              下載並更新
            </button>
            {u.info.notify && (
              <button className="min-h-10 rounded-lg px-4 text-neutral-400 hover:text-neutral-200" onClick={u.dismiss}>
                略過這版
              </button>
            )}
            <span className="text-xs text-neutral-500">
              更新會關閉聽譯{running ? ",執行中的回合會先停止" : ""};模型與設定不受影響。
            </span>
          </div>
        </article>
      )}
      {u.error && (
        <p className="text-sm text-rose-400" data-role="update-error">
          {u.error}
        </p>
      )}
    </section>
  );
}
