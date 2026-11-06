import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import RoutedModels from "./RoutedModels";
import type { Info, ObsStatus } from "./lib/types";

/** 「設定」分頁:所有模型的下載與選用、直播用網址。選用等同主視窗的下拉選單,下次按「開始」時載入。 */
export default function SettingsPage({
  info,
  running,
  model,
  onModel,
  asrEngine,
  onAsrEngine,
  onChanged,
}: {
  info: Info | null;
  running: boolean;
  model: string;
  onModel: (name: string) => void;
  asrEngine: string;
  onAsrEngine: (id: string) => void;
  onChanged: () => void;
}) {
  const dataDir = info?.data_dir;
  return (
    <div className="flex-1 overflow-y-auto p-4 flex flex-col gap-6 max-w-[860px]" data-role="settings-page">
      <p className="text-neutral-400 text-xs">
        在這裡下載模型並選擇要用哪一個;選好後回「字幕」分頁按「開始」才會載入。{running && "執行中不能換模型,停止後再選。"}
      </p>

      <section className="flex flex-col gap-2" data-section="translate">
        <h2 className="text-base font-semibold">翻譯模型</h2>
        <RoutedModels
          group="translate"
          running={running}
          dataDir={dataDir}
          onChanged={onChanged}
          title="llama.cpp 是翻譯伺服器(必要);其下每個翻譯模型擇一使用"
          activeKey={model}
          onUse={onModel}
        />
      </section>

      <section className="flex flex-col gap-2" data-section="base">
        <h2 className="text-base font-semibold">辨識基本模型</h2>
        <RoutedModels group="base" running={running} dataDir={dataDir} onChanged={onChanged} />
      </section>

      <section className="flex flex-col gap-3" data-section="engines">
        <h2 className="text-base font-semibold">辨識引擎</h2>
        {info?.asr_engines.map((e) => (
          <div key={e.id} className="flex flex-col gap-2" data-engine={e.id}>
            <div className="flex items-center gap-2">
              <span className="flex-1">
                {e.label}
                <span className="text-neutral-500 text-xs"> — {e.note}</span>
              </span>
              {e.present ? <span className="text-emerald-400 text-xs">可用</span> : <span className="text-neutral-500 text-xs">未下載</span>}
              {e.id === asrEngine ? (
                <span className="text-sky-300 text-xs" data-role="engine-active">
                  使用中
                </span>
              ) : (
                <button
                  className="rounded bg-neutral-800 px-2 py-0.5 text-xs hover:bg-neutral-700 disabled:opacity-40"
                  data-role="engine-use"
                  disabled={running}
                  onClick={() => onAsrEngine(e.id)}
                >
                  使用這個
                </button>
              )}
            </div>
            {e.id === "sensevoice" ? (
              <div className="text-neutral-500 text-xs">只需要上方的辨識基本模型。</div>
            ) : e.id === "routed" ? (
              <RoutedModels running={running} dataDir={dataDir} onChanged={onChanged} />
            ) : (
              <RoutedModels group="engine" engine={e.id} running={running} dataDir={dataDir} onChanged={onChanged} title="需要下載" />
            )}
          </div>
        ))}
      </section>

      <ObsCard />
    </div>
  );
}

const OBS_PREF = "tingyi.obs";

/** 直播用網址:開關、兩個網址與 OBS 設定提示。開關記在本機,下次開啟自動打開。 */
function ObsCard() {
  const [st, setSt] = useState<ObsStatus | null>(null);
  const [err, setErr] = useState("");
  const [copied, setCopied] = useState("");

  useEffect(() => {
    invoke<ObsStatus>("obs_status").then(setSt).catch(() => {});
  }, []);

  const toggle = async (enabled: boolean) => {
    setErr("");
    try {
      setSt(await invoke<ObsStatus>("set_obs_enabled", { enabled }));
      try {
        localStorage.setItem(OBS_PREF, enabled ? "1" : "0");
      } catch {
        // 存不了只影響下次是否自動打開
      }
    } catch (e) {
      setErr(String(e));
    }
  };

  const base = st?.port ? `http://127.0.0.1:${st.port}` : "";
  const urls = [
    { key: "history", label: "辨識紀錄(最近幾句原文+譯文)", url: `${base}/history?n=6&size=26`, hint: "建議 寬 800 × 高 600" },
    { key: "subtitle", label: "字幕(最新一句)", url: `${base}/subtitle?mode=translation&size=40`, hint: "建議 寬 1920 × 高 250" },
  ];
  const copy = async (key: string, url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(key);
      setTimeout(() => setCopied(""), 1500);
    } catch {
      setErr("無法複製,請手動選取網址");
    }
  };

  return (
    <section className="flex flex-col gap-2" data-section="obs">
      <h2 className="text-base font-semibold">直播(OBS 瀏覽器來源)</h2>
      <label className="flex items-center gap-2">
        {/* 狀態讀回前停用:否則看起來是關的,按下去送出的是「開」 */}
        <input type="checkbox" checked={!!st?.enabled} disabled={!st} onChange={(e) => toggle(e.target.checked)} data-role="obs-toggle" />
        開啟直播用網址(只有這台電腦能連)
      </label>
      {st?.enabled &&
        urls.map((u) => (
          <div key={u.key} className="flex flex-col gap-1 rounded border border-neutral-800 bg-neutral-900/50 px-2 py-2 text-xs">
            <div className="text-neutral-300">{u.label}</div>
            <div className="flex items-center gap-2">
              <input readOnly value={u.url} className="flex-1 bg-neutral-950 border border-neutral-700 rounded px-2 py-1 font-mono" data-role={`obs-url-${u.key}`} onFocus={(e) => e.target.select()} />
              <button className="rounded bg-sky-800 px-2 py-1 hover:bg-sky-700" onClick={() => copy(u.key, u.url)}>
                {copied === u.key ? "已複製" : "複製"}
              </button>
            </div>
            <div className="text-neutral-500">{u.hint}</div>
          </div>
        ))}
      {st?.enabled && (
        <p className="text-neutral-500 text-xs">
          OBS:來源「+」→「瀏覽器」→ 貼上網址,填寬高;背景是透明的。網址參數可改:n = 紀錄句數、size = 字級、mode=bilingual 字幕顯示原文。
        </p>
      )}
      {err && <p className="text-rose-400 text-xs">{err}</p>}
    </section>
  );
}

/** 主視窗開啟時:上次有打開就自動再打開。 */
export function restoreObs() {
  let on = false;
  try {
    on = localStorage.getItem(OBS_PREF) === "1";
  } catch {
    return;
  }
  if (on) invoke("set_obs_enabled", { enabled: true }).catch(() => {});
}
