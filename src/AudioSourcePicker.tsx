import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AudioSource, AudioSourceState, SourceEntry, SourceGroup } from "./lib/types";

const stem = (exe: string) => exe.replace(/\.exe$/i, "");
const TITLE_MAX = 70;
const GROUPS: [SourceGroup, string][] = [
  ["playing", "播放中"],
  ["windowed", "有視窗的程式"],
  ["background", "背景程式(沒有視窗)"],
];

const valueOf = (e: SourceEntry) => (e.kind === "window" ? `win:${e.hwnd}` : `app:${e.pid}`);

/** 音訊來源:系統全部聲音,或像 OBS「應用程式音訊擷取」那樣選一個視窗(擷取該程式含子程序);執行中也能切換。 */
export default function AudioSourcePicker({ running }: { running: boolean }) {
  const [entries, setEntries] = useState<SourceEntry[]>([]);
  const [state, setState] = useState<AudioSourceState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // 同一個程式有多個視窗時,記住使用者選的那個,下拉才顯示成他選的項目
  const [chosenHwnd, setChosenHwnd] = useState<number | null>(null);
  const lastRefresh = useRef(0);

  const refreshApps = useCallback(async () => {
    lastRefresh.current = Date.now();
    setLoading(true);
    try {
      setEntries(await invoke<SourceEntry[]>("list_audio_apps"));
      setError(null);
    } catch (e) {
      setError(`列舉程式失敗:${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, []);
  // 視窗標題會變(瀏覽器分頁):滑鼠移到下拉或取得焦點時自動更新,2 秒內不重複
  const refreshSoon = useCallback(() => {
    if (Date.now() - lastRefresh.current > 2000) refreshApps();
  }, [refreshApps]);
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
  // 停止後擷取執行緒已把 waiting 清掉,補讀一次才不會殘留「等它重新開啟」
  useEffect(() => {
    if (!running) refreshState();
  }, [running, refreshState]);

  const current = state?.source;
  // 目前綁定的程式不在清單(清單過期或已改綁)時仍列出,下拉才不會顯示成別的選項
  const options: SourceEntry[] =
    current?.kind === "app" && !entries.some((e) => e.pid === current.pid)
      ? [
          {
            kind: "background",
            name: stem(current.exe),
            group: "background",
            pid: current.pid,
            exe: current.exe,
            start_time: current.start_time,
            active: false,
          },
          ...entries,
        ]
      : entries;
  let value = "system";
  if (current?.kind === "app") {
    const mine = options.filter((e) => e.pid === current.pid);
    const pick = mine.find((e) => e.kind === "window" && e.hwnd === chosenHwnd) ?? mine[0];
    if (pick) value = valueOf(pick);
  }

  // 同一個執行檔有多個沒有視窗的獨立程序(例如兩個 powershell)時加註 pid,否則選項長得一樣
  const bgExe = options.filter((e) => e.kind === "background").map((e) => e.exe.toLowerCase());
  const dupExe = new Set(bgExe.filter((e, i) => bgExe.indexOf(e) !== i));
  const label = (e: SourceEntry) => {
    if (e.kind === "window") {
      const t = e.title.length > TITLE_MAX ? `${e.title.slice(0, TITLE_MAX)}…` : e.title;
      return `[${e.exe}]: ${t}`;
    }
    const name = e.name.toLowerCase() !== stem(e.exe).toLowerCase() ? `${e.name}(${e.exe})` : e.exe;
    return dupExe.has(e.exe.toLowerCase()) ? `${name} · pid ${e.pid}` : name;
  };

  const choose = async (v: string) => {
    let source: AudioSource = { kind: "system" };
    if (v !== "system") {
      const e = options.find((x) => valueOf(x) === v);
      if (!e) return;
      source = { kind: "app", pid: e.pid, exe: e.exe, start_time: e.start_time };
      setChosenHwnd(e.kind === "window" ? e.hwnd : null);
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
        onMouseEnter={refreshSoon}
        onFocus={refreshSoon}
      >
        <option value="system">系統全部聲音(預設輸出裝置)</option>
        {GROUPS.map(([g, name]) => {
          const items = options.filter((e) => e.group === g);
          return items.length === 0 ? null : (
            <optgroup key={g} label={name}>
              {items.map((e) => (
                <option key={valueOf(e)} value={valueOf(e)}>
                  {label(e)}
                </option>
              ))}
            </optgroup>
          );
        })}
      </select>
      <p className="text-neutral-500 text-xs">選任一視窗都會擷取該程式(含所有視窗、分頁與子程序)的聲音。</p>
      {state?.waiting && current?.kind === "app" && (
        <p className="text-amber-400 text-xs">{current.exe} 已關閉,等它重新開啟…</p>
      )}
      {entries.length === 0 && !loading && <p className="text-neutral-500 text-xs">找不到可擷取的程式。</p>}
      {error && <p className="text-red-400 text-xs">{error}</p>}
    </section>
  );
}
