import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppAudio, AudioSource, AudioSourceState } from "./lib/types";

const stem = (exe: string) => exe.replace(/\.exe$/i, "");

/** 音訊來源:系統全部聲音,或只擷取指定程式(含子程序);執行中也能切換。 */
export default function AudioSourcePicker({ running }: { running: boolean }) {
  const [apps, setApps] = useState<AppAudio[]>([]);
  const [state, setState] = useState<AudioSourceState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refreshApps = useCallback(async () => {
    setLoading(true);
    try {
      setApps(await invoke<AppAudio[]>("list_audio_apps"));
      setError(null);
    } catch (e) {
      setError(`列舉程式失敗:${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, []);
  // 讀不到只是暫時不更新顯示,下一輪會再讀,故不報錯。
  const refreshState = useCallback(
    () => invoke<AudioSourceState>("get_audio_source").then(setState).catch(() => {}),
    [],
  );

  useEffect(() => {
    refreshApps();
    refreshState();
  }, [refreshApps, refreshState]);
  // 執行中每 2 秒更新:目標程式重開後擷取執行緒會改綁新 pid,或進入等待
  useEffect(() => {
    if (!running) return;
    const t = setInterval(refreshState, 2000);
    return () => clearInterval(t);
  }, [running, refreshState]);

  const current = state?.source;
  const value = current?.kind === "app" ? `app:${current.pid}` : "system";
  // 目前綁定的程式不在清單(清單過期或已改綁)時仍列出,下拉才不會顯示成別的選項
  const options: AppAudio[] =
    current?.kind === "app" && !apps.some((a) => a.pid === current.pid)
      ? [{ pid: current.pid, exe: current.exe, name: stem(current.exe), active: false, start_time: current.start_time }, ...apps]
      : apps;

  // 同一個執行檔有多個獨立程序(例如兩個 powershell)時加註 pid,否則選項長得一樣
  const dupExe = new Set(options.map((a) => a.exe.toLowerCase()).filter((e, i, all) => all.indexOf(e) !== i));

  const choose = async (v: string) => {
    let source: AudioSource = { kind: "system" };
    if (v !== "system") {
      const a = options.find((x) => `app:${x.pid}` === v);
      if (!a) return;
      source = { kind: "app", pid: a.pid, exe: a.exe, start_time: a.start_time };
    }
    try {
      await invoke("set_audio_source", { source });
      await refreshState();
    } catch (e) {
      setError(`切換來源失敗:${String(e)}`);
    }
  };

  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center">
        <label className="text-neutral-400">音訊來源</label>
        <button
          className="ml-auto text-xs text-neutral-400 hover:text-neutral-200 disabled:opacity-50"
          onClick={refreshApps}
          disabled={loading}
          aria-label="重新整理程式清單"
        >
          {loading ? "讀取中…" : "重新整理"}
        </button>
      </div>
      <select
        className="bg-neutral-900 border border-neutral-700 rounded px-2 py-1"
        aria-label="音訊來源"
        value={value}
        onChange={(e) => choose(e.target.value)}
      >
        <option value="system">系統全部聲音(預設輸出裝置)</option>
        {options.map((a) => (
          <option key={a.pid} value={`app:${a.pid}`}>
            {a.name}
            {a.name.toLowerCase() !== stem(a.exe).toLowerCase() ? `(${a.exe})` : ""}
            {dupExe.has(a.exe.toLowerCase()) ? ` · pid ${a.pid}` : ""}
            {a.active ? " — 播放中" : ""}
          </option>
        ))}
      </select>
      {state?.waiting && current?.kind === "app" && (
        <p className="text-amber-400 text-xs">{current.exe} 已關閉,等它重新開啟…</p>
      )}
      {apps.length === 0 && !loading && (
        <p className="text-neutral-500 text-xs">清單只列出發過聲的程式;先讓目標程式播放一下再按「重新整理」。</p>
      )}
      {error && <p className="text-red-400 text-xs">{error}</p>}
    </section>
  );
}
