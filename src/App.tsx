import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSubtitles } from "./lib/useSubtitles";
import type { Info, OverlayMode, Sentence } from "./lib/types";

type LangOpt = "auto" | "en" | "ja" | "ko";

function pct(values: number[], p: number) {
  if (values.length === 0) return null;
  const v = [...values].sort((a, b) => a - b);
  return v[Math.min(v.length - 1, Math.round((v.length - 1) * p))];
}

export default function App() {
  const { sentences, errors, partial } = useSubtitles();
  const [info, setInfo] = useState<Info | null>(null);
  const [status, setStatus] = useState("就緒");
  const [busy, setBusy] = useState(false);
  const [running, setRunning] = useState(false);
  const [model, setModel] = useState<string>("Hy-MT2-1.8B-Q8_0");
  const [translate, setTranslate] = useState(true);
  const [asrEngine, setAsrEngine] = useState("sensevoice");
  const [lang, setLang] = useState<LangOpt>("auto");
  const [mode, setMode] = useState<OverlayMode>("translation");
  const [unlocked, setUnlocked] = useState(false);
  const [glossaryText, setGlossaryText] = useState("");
  const listRef = useRef<HTMLDivElement>(null);

  // get_info 只是狀態列資訊,失敗(例如後端尚未就緒)不影響操作,10 秒後會再查,故不報錯。
  const refreshInfo = () => invoke<Info>("get_info").then(setInfo).catch(() => {});
  const autostarted = useRef(false);
  const modelDefaulted = useRef(false);

  useEffect(() => {
    // 首次取得資訊時,以後端依顯示記憶體建議的模型當預設;之後使用者可自行更改
    if (info?.recommended_model && !modelDefaulted.current) {
      modelDefaulted.current = true;
      setModel(info.recommended_model);
    }
  }, [info?.recommended_model]);

  useEffect(() => {
    // 開發用自動開始(TINGYI_AUTOSTART=1):只在 autostart 旗標首次為 true 時觸發一次;
    // 刻意不把 start / info 放進依賴,否則每次 info 刷新都會重跑這個 effect。
    if (info?.autostart && !info.running && !autostarted.current) {
      autostarted.current = true;
      void start();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [info?.autostart]);

  useEffect(() => {
    refreshInfo();
    const un = listen<string>("status", (e) => setStatus(e.payload));
    const t = setInterval(refreshInfo, 10000);
    return () => {
      un.then((f) => f());
      clearInterval(t);
    };
  }, []);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [sentences.length, sentences[sentences.length - 1]?.translation, partial]);

  const stats = useMemo(() => {
    const done = sentences.filter((s) => s.timing);
    const e2e = done.map((s) => s.timing!.e2e_first_ms!).filter((x) => x != null);
    const asr = sentences.map((s) => s.asrMs).filter((x) => x > 0);
    const mtFirst = done.map((s) => s.timing!.mt_first_ms!).filter((x) => x != null);
    return {
      n: sentences.length,
      asrP50: pct(asr, 0.5),
      mtFirstP50: pct(mtFirst, 0.5),
      e2eP50: pct(e2e, 0.5),
      e2eP95: pct(e2e, 0.95),
      last: done.length ? done[done.length - 1].timing : undefined,
    };
  }, [sentences]);

  const parseGlossary = () =>
    glossaryText
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith("#"))
      .map((l) => {
        const [source, target] = l.split(/\t|=|→/).map((x) => x?.trim() ?? "");
        return { source, target };
      })
      .filter((g) => g.source && g.target);

  const start = async () => {
    setBusy(true);
    try {
      await invoke("start_session", {
        opts: {
          model: translate ? model : null,
          asr_engine: asrEngine,
          lang: lang === "auto" ? null : lang,
          glossary: parseGlossary(),
        },
      });
      setRunning(true);
    } catch (e) {
      setStatus(`啟動失敗:${String(e)}`);
    } finally {
      setBusy(false);
      refreshInfo();
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      await invoke("stop_session");
      setRunning(false);
    } catch (e) {
      setStatus(`停止失敗:${String(e)}`);
    } finally {
      setBusy(false);
      refreshInfo();
    }
  };

  const toggleUnlock = async () => {
    const next = !unlocked;
    await invoke("set_overlay_interactive", { interactive: next });
    setUnlocked(next);
  };

  const changeMode = async (m: OverlayMode) => {
    setMode(m);
    await invoke("set_overlay_mode", { mode: m });
  };

  const ready = info && info.sensevoice_ready && info.vad_ready && (!translate || info.llama_server_ready);

  return (
    <div className="h-screen flex flex-col text-sm">
      <header className="px-4 py-3 border-b border-neutral-800 flex items-center gap-3">
        <h1 className="text-lg font-semibold tracking-wide">聽譯</h1>
        <span className="text-neutral-400">技術驗證版</span>
        <span className="ml-auto text-neutral-300 truncate max-w-[50%]" title={status}>
          {status}
        </span>
      </header>

      <div className="flex flex-1 min-h-0">
        {/* 左:控制 */}
        <aside className="w-[320px] shrink-0 border-r border-neutral-800 p-4 flex flex-col gap-4 overflow-y-auto">
          <section className="flex flex-col gap-2">
            <label className="text-neutral-400">翻譯模型</label>
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={translate} onChange={(e) => setTranslate(e.target.checked)} disabled={running} />
              啟用本機翻譯(llama-server)
            </label>
            <select
              className="bg-neutral-900 border border-neutral-700 rounded px-2 py-1 disabled:opacity-50"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              disabled={running || !translate}
            >
              {info?.models.map((m) => (
                <option key={m.name} value={m.name} disabled={!m.present}>
                  {m.name} {m.present ? `(${(m.size_mb / 1000).toFixed(1)} GB)` : "(未下載)"} — {m.note}
                </option>
              ))}
            </select>
          </section>

          <section className="flex flex-col gap-2">
            <label className="text-neutral-400">辨識引擎</label>
            <select
              className="bg-neutral-900 border border-neutral-700 rounded px-2 py-1 disabled:opacity-50"
              value={asrEngine}
              onChange={(e) => setAsrEngine(e.target.value)}
              disabled={running}
            >
              {info?.asr_engines.map((e) => (
                <option key={e.id} value={e.id} disabled={!e.present}>
                  {e.label} — {e.note}
                </option>
              ))}
            </select>
          </section>

          <section className="flex flex-col gap-2">
            <label className="text-neutral-400">來源語言</label>
            <div className="flex gap-1">
              {(["auto", "en", "ja", "ko"] as LangOpt[]).map((l) => (
                <button
                  key={l}
                  disabled={running}
                  onClick={() => setLang(l)}
                  className={`flex-1 rounded px-2 py-1 border ${
                    lang === l ? "bg-sky-700 border-sky-500" : "bg-neutral-900 border-neutral-700"
                  } disabled:opacity-50`}
                >
                  {l === "auto" ? "自動" : l}
                </button>
              ))}
            </div>
          </section>

          <section className="flex flex-col gap-2">
            <label className="text-neutral-400">術語表(每行 原文=譯文)</label>
            <textarea
              className="bg-neutral-900 border border-neutral-700 rounded px-2 py-1 h-24 font-mono text-xs"
              value={glossaryText}
              onChange={(e) => setGlossaryText(e.target.value)}
              disabled={running}
              placeholder={"Exile=流亡者\nWraeclast=瓦爾克拉斯"}
            />
          </section>

          <section className="flex gap-2">
            {!running ? (
              <button
                className="flex-1 bg-emerald-700 hover:bg-emerald-600 disabled:opacity-50 rounded py-2 font-semibold"
                onClick={start}
                disabled={busy || !ready}
              >
                {busy ? "啟動中…" : "開始"}
              </button>
            ) : (
              <button
                className="flex-1 bg-rose-700 hover:bg-rose-600 disabled:opacity-50 rounded py-2 font-semibold"
                onClick={stop}
                disabled={busy}
              >
                {busy ? "停止中…" : "停止"}
              </button>
            )}
          </section>

          <section className="flex flex-col gap-2">
            <label className="text-neutral-400">懸浮字幕</label>
            <div className="flex gap-1">
              <button
                onClick={() => changeMode("bilingual")}
                className={`flex-1 rounded px-2 py-1 border ${mode === "bilingual" ? "bg-sky-700 border-sky-500" : "bg-neutral-900 border-neutral-700"}`}
              >
                雙語
              </button>
              <button
                onClick={() => changeMode("translation")}
                className={`flex-1 rounded px-2 py-1 border ${mode === "translation" ? "bg-sky-700 border-sky-500" : "bg-neutral-900 border-neutral-700"}`}
              >
                只看譯文
              </button>
            </div>
            <button
              onClick={toggleUnlock}
              className={`rounded px-2 py-1 border ${unlocked ? "bg-yellow-700 border-yellow-500" : "bg-neutral-900 border-neutral-700"}`}
            >
              {unlocked ? "鎖定位置(恢復滑鼠穿透)" : "解鎖以拖曳位置"}
            </button>
          </section>

          <section className="text-xs text-neutral-400 flex flex-col gap-1 mt-auto">
            <div>輸出裝置:{info?.device ?? "—"}</div>
            <div>
              GPU:{info?.gpu ? `${info.gpu[0]},已用 ${info.gpu_used_mib ?? "?"} / ${info.gpu[1]} MiB` : "—"}
            </div>
            <div>
              模型:SenseVoice {info?.sensevoice_ready ? "✓" : "✗"} · VAD {info?.vad_ready ? "✓" : "✗"} · llama-server{" "}
              {info?.llama_server_ready ? "✓" : "✗"}
            </div>
            <div className="truncate" title={info?.data_dir}>
              資料:{info?.data_dir}
            </div>
          </section>
        </aside>

        {/* 右:紀錄與指標 */}
        <main className="flex-1 flex flex-col min-w-0">
          <div className="px-4 py-2 border-b border-neutral-800 grid grid-cols-5 gap-2 text-xs">
            <Metric label="句數" value={stats.n} />
            <Metric label="辨識 p50" value={stats.asrP50} unit="ms" />
            <Metric label="譯文首字 p50" value={stats.mtFirstP50} unit="ms" />
            <Metric label="端到端首字 p50" value={stats.e2eP50} unit="ms" warn={(stats.e2eP50 ?? 0) > 2000} />
            <Metric label="端到端首字 p95" value={stats.e2eP95} unit="ms" warn={(stats.e2eP95 ?? 0) > 2000} />
          </div>
          <div ref={listRef} className="flex-1 overflow-y-auto px-4 py-2 flex flex-col gap-2">
            {sentences.length === 0 && (
              <div className="text-neutral-500 mt-8 text-center">
                按「開始」後播放英文、日文或韓文影片,字幕會出現在這裡與懸浮視窗。
              </div>
            )}
            {sentences.map((s) => (
              <SentenceRow key={s.id} s={s} />
            ))}
            {partial && (
              <div className="rounded border border-dashed border-neutral-800 px-3 py-2 text-neutral-500 italic">
                講話中:{partial}
              </div>
            )}
          </div>
          {errors.length > 0 && (
            <div className="px-4 py-1 border-t border-rose-900 text-rose-300 text-xs truncate">{errors[errors.length - 1]}</div>
          )}
        </main>
      </div>
    </div>
  );
}

function Metric({ label, value, unit, warn }: { label: string; value: number | null | undefined; unit?: string; warn?: boolean }) {
  return (
    <div className="bg-neutral-900 rounded px-2 py-1">
      <div className="text-neutral-500">{label}</div>
      <div className={`text-base font-mono ${warn ? "text-amber-400" : "text-neutral-100"}`}>
        {value == null ? "—" : value}
        {value != null && unit ? <span className="text-neutral-500 text-xs"> {unit}</span> : null}
      </div>
    </div>
  );
}

function SentenceRow({ s }: { s: Sentence }) {
  const t = s.timing;
  return (
    <div className="rounded border border-neutral-800 bg-neutral-900/60 px-3 py-2">
      <div className="flex items-baseline gap-2 text-xs text-neutral-500">
        <span>#{s.id}</span>
        <span className="uppercase">{s.lang}</span>
        <span>辨識 {s.asrMs} ms</span>
        {t && (
          <span>
            · 譯文首字 {t.mt_first_ms ?? "—"} ms · 端到端 {t.e2e_first_ms ?? "—"} ms
          </span>
        )}
        {s.state === "failed" && <span className="text-amber-400">翻譯失敗:{s.error}</span>}
        {s.state === "no_translate" && <span>不翻譯</span>}
      </div>
      <div className="text-neutral-300">{s.source}</div>
      {s.state !== "no_translate" && (
        <div className={`text-base ${s.state === "translating" ? "text-neutral-400" : "text-white"}`}>
          {s.translation || (s.state === "translating" ? "…" : "")}
        </div>
      )}
    </div>
  );
}
