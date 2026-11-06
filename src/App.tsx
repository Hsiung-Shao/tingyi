import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { useSubtitles } from "./lib/useSubtitles";
import type { Info, OverlayMode, Prefs, Sensitivity, Sentence } from "./lib/types";
import { speakerColor } from "./lib/speakerColor";
import PerfPanel from "./PerfPanel";
import AudioSourcePicker from "./AudioSourcePicker";
import SettingsPage, { type Section } from "./SettingsPage";
import { UpdateHint, useUpdater } from "./Updater";
import WebTranslateCard from "./WebTranslateCard";

type LangOpt = "auto" | "en" | "ja" | "ko";

function pct(values: number[], p: number) {
  if (values.length === 0) return null;
  const v = [...values].sort((a, b) => a - b);
  return v[Math.min(v.length - 1, Math.round((v.length - 1) * p))];
}

export default function App() {
  const { sentences, errors, partial, partialSpeaker, speakers, segmentation } = useSubtitles();
  const [info, setInfo] = useState<Info | null>(null);
  const [status, setStatus] = useState("就緒");
  const [busy, setBusy] = useState(false);
  const [running, setRunning] = useState(false);
  const [model, setModel] = useState<string>("Hy-MT2-1.8B-Q8_0");
  const [translate, setTranslate] = useState(true);
  const [asrEngine, setAsrEngine] = useState("sensevoice");
  const [lang, setLang] = useState<LangOpt>("auto");
  const [targetLang, setTargetLang] = useState("zh-TW");
  const [zhConvert, setZhConvert] = useState("terms");
  const [mode, setMode] = useState<OverlayMode>("translation");
  const [unlocked, setUnlocked] = useState(false);
  const [sensitivity, setSensitivity] = useState<Sensitivity>("normal");
  const [adaptive, setAdaptive] = useState(true);
  /** 手動單段上限(秒);空字串 = 依快/中/慢預設 */
  const [maxSpeech, setMaxSpeech] = useState("");
  /** 懸浮字幕沒聲音幾秒後收起;空字串 = 3 秒,0 = 不隱藏 */
  const [hideSec, setHideSec] = useState("3");
  const [speakerId, setSpeakerId] = useState(true);
  const [refine, setRefine] = useState(true);
  const [renaming, setRenaming] = useState<{ from: string; to: string } | null>(null);
  const [tab, setTab] = useState<"main" | "settings">("main");
  const [settingsSection, setSettingsSection] = useState<Section>("translate");
  const updater = useUpdater();

  const applySegmentation = async (s: Sensitivity, a: boolean, m: string = maxSpeech) => {
    setSensitivity(s);
    setAdaptive(a);
    const sec = Number.parseFloat(m);
    await invoke("set_segmentation", { sensitivity: s, adaptive: a, maxSpeechSec: Number.isFinite(sec) && sec > 0 ? sec : null });
  };

  const applySubtitleHide = async () => {
    const sec = Number.parseFloat(hideSec);
    // 後端會夾在 0 或 1–30 秒,輸入框改顯示實際生效的值
    const applied = await invoke<number>("set_subtitle_hide", { seconds: Number.isFinite(sec) ? sec : 3 });
    setHideSec(String(applied));
  };

  const commitRename = async () => {
    if (!renaming) return;
    const { from, to } = renaming;
    setRenaming(null);
    if (to.trim() && to.trim() !== from) {
      try {
        await invoke("rename_speaker", { from, to: to.trim() });
      } catch (e) {
        setStatus(`改名失敗:${String(e)}`);
      }
    }
  };
  const [glossaryText, setGlossaryText] = useState("");
  const [replaceText, setReplaceText] = useState("");
  const listRef = useRef<HTMLDivElement>(null);

  // get_info 只是狀態列資訊,失敗(例如後端尚未就緒)不影響操作,10 秒後會再查,故不報錯。
  // 固定參照:設定頁的區塊用 memo,回呼每次重繪都換新會讓它們跟著字幕重繪
  const refreshInfo = useCallback(() => invoke<Info>("get_info").then(setInfo).catch(() => {}), []);
  const chooseModel = useCallback((m: string) => {
    setModel(m);
    setTranslate(true);
  }, []);
  const autostarted = useRef(false);
  const modelDefaulted = useRef(false);

  useEffect(() => {
    // 首次取得資訊時,以後端依顯示記憶體建議的模型當預設;之後使用者可自行更改
    if (info?.recommended_model && !modelDefaulted.current) {
      modelDefaulted.current = true;
      setModel(info.recommended_model);
    }
  }, [info?.recommended_model]);

  // 開始選項存進 settings.json:下次開啟沿用,擴充在背景喚起聽譯時也照這些選項開始
  const [prefsLoaded, setPrefsLoaded] = useState(false);
  useEffect(() => {
    invoke<Prefs | null>("get_prefs")
      .then((p) => {
        if (!p) return;
        modelDefaulted.current = true;
        setModel(p.model);
        setTranslate(p.translate);
        setAsrEngine(p.asr_engine);
        setLang(p.lang as LangOpt);
        setSpeakerId(p.speaker_id);
        setRefine(p.refine);
        setGlossaryText(p.glossary);
        setReplaceText(p.replacements);
        if (p.target_lang) setTargetLang(p.target_lang);
        if (p.zh_convert) setZhConvert(p.zh_convert);
      })
      .catch(() => {})
      .finally(() => setPrefsLoaded(true));
  }, []);
  useEffect(() => {
    // 讀回之前不寫,免得用預設值蓋掉存檔;打字時等停下來再寫
    if (!prefsLoaded) return;
    const prefs: Prefs = {
      model,
      translate,
      asr_engine: asrEngine,
      lang,
      speaker_id: speakerId,
      refine,
      glossary: glossaryText,
      replacements: replaceText,
      target_lang: targetLang,
      zh_convert: zhConvert,
    };
    const t = setTimeout(() => invoke("save_prefs", { prefs }).catch(() => {}), 500);
    return () => clearTimeout(t);
  }, [prefsLoaded, model, translate, asrEngine, lang, speakerId, refine, glossaryText, replaceText, targetLang, zhConvert]);

  useEffect(() => {
    // 開發用自動開始(TINGYI_AUTOSTART=1):只在 autostart 旗標首次為 true 時觸發一次;
    // 刻意不把 start / info 放進依賴,否則每次 info 刷新都會重跑這個 effect。
    if (info?.autostart && !info.running && !autostarted.current) {
      autostarted.current = true;
      void start(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [info?.autostart]);

  useEffect(() => {
    refreshInfo();
    const un = listen<string>("status", (e) => setStatus(e.payload));
    // 後端停止時會自動退出拖曳模式,按鈕狀態跟著同步
    const un2 = listen<boolean>("overlay-interactive", (e) => setUnlocked(e.payload));
    // 回合也可能由後端開始或停止(擴充觸發),按鈕狀態跟著後端
    const un3 = listen<boolean>("session-running", (e) => setRunning(e.payload));
    const t = setInterval(refreshInfo, 10000);
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
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

  // 辨識取代表:每行 錯字=正字(也接受 Tab 或 →),# 開頭為註解
  const parseReplacements = () =>
    replaceText
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith("#"))
      .map((l) => {
        // 只切第一個分隔符;沒有分隔符的行略過,右側留空才是明確的刪除規則
        const m = l.match(/^(.*?)(?:\t|=|→)(.*)$/);
        return m ? { from: m[1].trim(), to: m[2].trim() } : { from: "", to: "" };
      })
      .filter((r) => r.from);

  const glossaryCount = parseGlossary().length;
  const replaceCount = parseReplacements().length;

  const [askClear, setAskClear] = useState(false);

  const start = async (clearHistory?: boolean) => {
    // 有上一輪紀錄且尚未決定 → 先問要清除還是保留
    if (clearHistory === undefined && sentences.length > 0) {
      setAskClear(true);
      return;
    }
    setAskClear(false);
    setBusy(true);
    try {
      await invoke("start_session", {
        opts: {
          model: translate ? model : null,
          asr_engine: asrEngine,
          speaker_id: speakerId,
          refine: refine && asrEngine === "whisper-turbo",
          clear_history: clearHistory ?? true,
          lang: lang === "auto" ? null : lang,
          glossary: parseGlossary(),
          replacements: parseReplacements(),
          target_lang: targetLang,
          zh_convert: zhConvert,
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
    try {
      await invoke("set_overlay_interactive", { interactive: next });
      setUnlocked(next);
    } catch (e) {
      setStatus(`切換字幕位置模式失敗:${String(e)}`);
    }
  };

  const exportTranscript = async () => {
    const d = new Date();
    const p2 = (n: number) => String(n).padStart(2, "0");
    const stamp = `${d.getFullYear()}${p2(d.getMonth() + 1)}${p2(d.getDate())}-${p2(d.getHours())}${p2(d.getMinutes())}`;
    try {
      const path = await save({
        defaultPath: `聽譯-${stamp}.txt`,
        filters: [{ name: "文字檔", extensions: ["txt"] }],
      });
      if (!path) return; // 使用者取消
      const n = await invoke<number>("export_transcript", {
        path,
        exportedAt: `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${p2(d.getHours())}:${p2(d.getMinutes())}`,
        sentences: sentences.map((s) => ({
          id: s.id,
          source: s.source,
          translation: s.translation,
          state: s.state,
          lang: s.lang,
          speaker: s.speaker,
          start_ms: s.startMs ?? null,
          revised: !!s.revised,
          recovered: !!s.recovered,
        })),
      });
      setStatus(`已匯出 ${n} 句:${path}`);
    } catch (e) {
      setStatus(`匯出失敗:${String(e)}`);
    }
  };

  const changeMode = async (m: OverlayMode) => {
    setMode(m);
    await invoke("set_overlay_mode", { mode: m });
  };

  const engine = info?.asr_engines.find((e) => e.id === asrEngine);
  const cohereNoLang = asrEngine === "cohere" && lang === "auto";
  const mtModel = info?.models.find((m) => m.name === model);
  const ready =
    info &&
    info.sensevoice_ready &&
    info.vad_ready &&
    (!translate || info.llama_server_ready) &&
    (engine?.present ?? true) &&
    (!translate || (mtModel?.present ?? true)) &&
    !cohereNoLang;
  // 「開始」不能按時直接說缺什麼(新電腦第一次使用常見)
  const missing = info
    ? [
        !info.sensevoice_ready && "SenseVoice 辨識模型",
        !info.vad_ready && "VAD 語音偵測",
        translate && !info.llama_server_ready && "llama.cpp 翻譯伺服器",
        translate && mtModel && !mtModel.present && `${model} 翻譯模型`,
        engine && !engine.present && `${engine.label} 的模型`,
      ].filter(Boolean)
    : [];

  return (
    <div className="h-screen flex flex-col text-sm">
      {askClear && (
        <div className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center">
          <div className="bg-neutral-900 border border-neutral-700 rounded-lg p-5 w-[360px] flex flex-col gap-4">
            <div className="text-base font-semibold">上一輪還有 {sentences.length} 句紀錄</div>
            <div className="text-neutral-400">要清除後重新開始,還是保留並接續?</div>
            <div className="flex gap-2">
              <button className="flex-1 bg-rose-700 hover:bg-rose-600 rounded py-2" onClick={() => start(true)}>
                清除
              </button>
              <button className="flex-1 bg-sky-700 hover:bg-sky-600 rounded py-2" onClick={() => start(false)}>
                保留
              </button>
              <button className="px-3 bg-neutral-800 hover:bg-neutral-700 rounded py-2" onClick={() => setAskClear(false)}>
                取消
              </button>
            </div>
          </div>
        </div>
      )}
      <header className="px-4 py-3 border-b border-neutral-800 flex items-center gap-3">
        <h1 className="text-lg font-semibold tracking-wide">聽譯</h1>
        <span className="text-neutral-400">技術驗證版</span>
        <nav className="flex gap-1 ml-4" data-role="tabs">
          {(
            [
              ["main", "字幕"],
              ["settings", "設定"],
            ] as const
          ).map(([k, label]) => (
            <button
              key={k}
              data-tab={k}
              className={`rounded px-3 py-1 ${tab === k ? "bg-neutral-700 text-white" : "text-neutral-400 hover:bg-neutral-800"}`}
              onClick={() => setTab(k)}
            >
              {label}
            </button>
          ))}
        </nav>
        <span className="ml-auto">
          <UpdateHint
            u={updater}
            running={running}
            onOpen={() => {
              setTab("settings");
              setSettingsSection("about");
            }}
          />
        </span>
        <span className="flex items-center gap-2 text-neutral-300 truncate max-w-[50%]" title={status}>
          {running && <span className="h-2 w-2 shrink-0 rounded-full bg-emerald-400" aria-hidden="true" />}
          {status}
        </span>
      </header>

      {/* 兩個分頁都保持掛載,只藏起來:下載進度、紀錄捲動與輸入框內容都保留 */}
      <div className={tab === "settings" ? "flex flex-1 min-h-0" : "hidden"}>
        <SettingsPage
          sentences={sentences}
          section={settingsSection}
          onSection={setSettingsSection}
          updater={updater}
          info={info}
          running={running}
          model={model}
          onModel={chooseModel}
          asrEngine={asrEngine}
          onAsrEngine={setAsrEngine}
          onChanged={refreshInfo}
        />
      </div>
      <div className={tab === "main" ? "flex flex-1 min-h-0" : "hidden"}>
        {/* 左:控制 */}
        <aside className="w-[340px] shrink-0 border-r border-neutral-800 px-4 py-5 flex flex-col gap-3 overflow-y-auto">
          {/* 開始:主要動作、音訊來源、目前用的引擎與模型 */}
          <Card>
            {!running ? (
              <button
                className="min-h-12 rounded-[10px] bg-emerald-700 text-base font-bold hover:bg-emerald-600 disabled:opacity-50"
                onClick={() => start()}
                disabled={busy || !ready}
              >
                {busy ? "啟動中…" : "開始"}
              </button>
            ) : (
              <button
                className="min-h-12 rounded-[10px] bg-rose-700 text-base font-bold hover:bg-rose-600 disabled:opacity-50"
                onClick={stop}
                disabled={busy}
              >
                {busy ? "停止中…" : "停止"}
              </button>
            )}
            {!running && (missing.length > 0 || cohereNoLang) && (
              <p className="text-amber-400 text-xs" data-role="start-missing">
                還不能開始:
                {missing.length > 0 && `缺少${missing.join("、")}。請到「設定」分頁下載。`}
                {cohereNoLang && "Cohere 要在「來源語言」選 en、ja 或 ko。"}
              </p>
            )}
            <AudioSourcePicker running={running} />
            <div className="flex flex-col gap-2 rounded-lg bg-neutral-800/50 px-3 py-2.5 text-[13px]">
              <label className="flex items-center gap-2">
                <span className="w-9 shrink-0 text-neutral-400">辨識</span>
                <select
                  className="min-w-0 flex-1 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 disabled:opacity-50"
                  value={asrEngine}
                  onChange={(e) => setAsrEngine(e.target.value)}
                  disabled={running}
                >
                  {info?.asr_engines.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.label} — {e.note}
                      {e.present ? "" : "(未下載)"}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex items-center gap-2">
                <span className="w-9 shrink-0 text-neutral-400">翻譯</span>
                <select
                  className="min-w-0 flex-1 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 disabled:opacity-50"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  disabled={running || !translate}
                >
                  {info?.models.map((m) => (
                    <option key={m.name} value={m.name}>
                      {m.name} {m.present ? `(${(m.size_mb / 1000).toFixed(1)} GB)` : "(未下載)"} — {m.note}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex items-center gap-2 text-neutral-300">
                <input type="checkbox" className="accent-sky-400" checked={translate} onChange={(e) => setTranslate(e.target.checked)} disabled={running} />
                啟用本機翻譯
              </label>
              {info && (!info.sensevoice_ready || !info.vad_ready || (engine && !engine.present)) && (
                <GoSettings onClick={() => setTab("settings")} what="辨識模型還沒下載" />
              )}
              {translate && info && (!info.llama_server_ready || (mtModel && !mtModel.present)) && (
                <GoSettings onClick={() => setTab("settings")} what="翻譯伺服器或這個模型還沒下載" />
              )}
              <button className="self-end text-xs text-sky-300 hover:text-sky-200" onClick={() => setTab("settings")}>
                到設定下載或換模型 →
              </button>
            </div>
          </Card>

          <Card title="語言">
            <div className="flex flex-col gap-1.5">
              <span className="text-xs text-neutral-400">來源語言</span>
              <Segmented
                options={(["auto", "en", "ja", "ko"] as LangOpt[]).map((l) => ({ value: l, label: l === "auto" ? "自動" : l }))}
                value={lang}
                onChange={(l) => setLang(l as LangOpt)}
                disabled={running}
              />
            </div>
            <div className="grid grid-cols-2 gap-2">
              <label className="flex flex-col gap-1.5 text-xs text-neutral-400" htmlFor="target-lang">
                翻譯成
                <select
                  id="target-lang"
                  className="min-h-9 rounded-lg border border-neutral-700 bg-neutral-950 px-2 text-[13px] text-neutral-200 disabled:opacity-50"
                  value={targetLang}
                  onChange={(e) => setTargetLang(e.target.value)}
                  disabled={running || !translate}
                >
                  <option value="zh-TW">繁體中文</option>
                  <option value="zh-CN">簡體中文</option>
                  <option value="en">English</option>
                  <option value="ja">日本語</option>
                  <option value="ko">한국어</option>
                </select>
              </label>
              {targetLang === "zh-TW" && (
                <label className="flex flex-col gap-1.5 text-xs text-neutral-400" htmlFor="zh-convert">
                  譯文轉換
                  <select
                    id="zh-convert"
                    className="min-h-9 rounded-lg border border-neutral-700 bg-neutral-950 px-2 text-[13px] text-neutral-200 disabled:opacity-50"
                    value={zhConvert}
                    onChange={(e) => setZhConvert(e.target.value)}
                    disabled={running || !translate}
                  >
                    <option value="terms">台灣繁體</option>
                    <option value="script">只轉字形</option>
                    <option value="none">不轉換</option>
                  </select>
                </label>
              )}
            </div>
            {/* 只有 Hy-MT2 能譯成中文以外的語言,其他模型後端會改用繁體中文 */}
            {targetLang !== "zh-TW" && !model.startsWith("Hy-MT2") && (
              <p className="text-amber-400 text-xs">這個模型只能譯成繁體中文,開始時會改用繁體中文。</p>
            )}
          </Card>

          <Card
            title="斷句"
            hint={segmentation ? `靜音 ${segmentation.min_silence_ms} ms · 上限 ${segmentation.max_speech_ms / 1000} 秒(${segmentation.reason})` : undefined}
          >
            <Segmented
              options={(["fast", "normal", "slow"] as Sensitivity[]).map((s) => ({
                value: s,
                label: s === "fast" ? "快" : s === "normal" ? "中" : "慢",
                title: s === "fast" ? "停頓短也切,翻譯等待較短(可能較零碎)" : undefined,
              }))}
              value={sensitivity}
              onChange={(s) => applySegmentation(s as Sensitivity, adaptive)}
            />
            <div className="flex items-center gap-2 text-[13px]">
              <label className="flex items-center gap-1.5">
                <input type="checkbox" className="accent-sky-400" checked={adaptive} onChange={(e) => applySegmentation(sensitivity, e.target.checked)} />
                依語速自動調整
              </label>
              <label
                className="ml-auto flex items-center gap-1.5 text-neutral-400"
                title="連續講話最多等幾秒就硬切;越小越快、辨識越不準。空白 = 依快/中/慢(3 / 8 / 10 秒)"
              >
                單段上限
                <input
                  type="number"
                  aria-label="單段上限(秒)"
                  min={2}
                  max={15}
                  step={0.5}
                  value={maxSpeech}
                  placeholder={sensitivity === "fast" ? "3" : sensitivity === "slow" ? "10" : "8"}
                  onChange={(e) => setMaxSpeech(e.target.value)}
                  onBlur={() => applySegmentation(sensitivity, adaptive)}
                  onKeyDown={(e) => e.key === "Enter" && applySegmentation(sensitivity, adaptive)}
                  className="w-14 rounded-md border border-neutral-700 bg-neutral-950 px-1.5 py-1 text-neutral-200"
                />
                秒
              </label>
            </div>
          </Card>

          <Card title="懸浮字幕">
            <Segmented
              options={[
                { value: "translation", label: "只看譯文" },
                { value: "bilingual", label: "雙語" },
              ]}
              value={mode}
              onChange={(m) => changeMode(m as OverlayMode)}
            />
            <div className="flex items-center gap-2 text-[13px]">
              <button
                onClick={toggleUnlock}
                className={`min-h-8 rounded-md border px-3 ${unlocked ? "border-yellow-500 bg-yellow-700" : "border-neutral-700 bg-neutral-800 hover:bg-neutral-700"}`}
              >
                {unlocked ? "鎖定位置(恢復滑鼠穿透)" : "解鎖以拖曳位置"}
              </button>
              <label
                className="ml-auto flex items-center gap-1.5 text-neutral-400"
                title="沒聲音或沒人講話超過這個秒數(且譯文已顯示這麼久),懸浮字幕自動收起;有人再講話才出現新字幕。0 = 不隱藏"
              >
                沒聲音
                <input
                  type="number"
                  aria-label="沒聲音幾秒後隱藏字幕"
                  min={0}
                  max={30}
                  step={0.5}
                  value={hideSec}
                  placeholder="3"
                  onChange={(e) => setHideSec(e.target.value)}
                  onBlur={applySubtitleHide}
                  onKeyDown={(e) => e.key === "Enter" && applySubtitleHide()}
                  className="w-12 rounded-md border border-neutral-700 bg-neutral-950 px-1.5 py-1 text-neutral-200"
                />
                秒收起
              </label>
            </div>
          </Card>

          {/* 不常改的設定收成可展開的列;內容一直在頁面上,只是收起來 */}
          <div className="flex flex-col overflow-hidden rounded-xl border border-neutral-800 bg-neutral-900/60">
            <Fold title="說話者" meta={speakerId ? (speakers.length > 0 ? `開啟 · ${speakers.length} 位` : "開啟") : "關閉"}>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  className="accent-sky-400"
                  checked={speakerId}
                  onChange={(e) => setSpeakerId(e.target.checked)}
                  disabled={running || !info?.speaker_model_ready}
                />
                辨識說話者{info && !info.speaker_model_ready ? "(模型未下載)" : ""}
              </label>
              {speakers.length > 0 && (
                <div className="flex flex-wrap gap-1 text-xs">
                  {speakers.map((n) =>
                    renaming?.from === n ? (
                      <input
                        key={n}
                        autoFocus
                        className="w-28 rounded border border-sky-500 bg-neutral-950 px-1"
                        value={renaming.to}
                        onChange={(e) => setRenaming({ from: n, to: e.target.value })}
                        onBlur={commitRename}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void commitRename();
                          if (e.key === "Escape") setRenaming(null);
                        }}
                      />
                    ) : (
                      <button
                        key={n}
                        title="點一下改名"
                        onClick={() => setRenaming({ from: n, to: n })}
                        className={`rounded border border-neutral-700 bg-neutral-950 px-2 py-0.5 ${speakerColor(n, speakers)}`}
                      >
                        {n}
                      </button>
                    ),
                  )}
                </div>
              )}
            </Fold>
            <Fold title="回修" meta={asrEngine !== "whisper-turbo" ? "僅 Whisper" : refine ? "開啟" : "關閉"}>
              <label className="flex items-center gap-2" title="幾秒後用更長的連續音訊重新辨識、補回漏掉的句子,並帶前後文重翻;歷史列表以「修」「補」標示">
                <input
                  type="checkbox"
                  className="accent-sky-400"
                  checked={refine && asrEngine === "whisper-turbo"}
                  onChange={(e) => setRefine(e.target.checked)}
                  disabled={running || asrEngine !== "whisper-turbo"}
                />
                慢路徑回修原文與譯文{asrEngine !== "whisper-turbo" ? "(僅 Whisper 引擎)" : ""}
              </label>
            </Fold>
            <Fold title="術語表" meta={glossaryCount > 0 ? `${glossaryCount} 條` : "未設定"}>
              <span className="text-xs text-neutral-500">每行 原文=譯文</span>
              <textarea
                className="h-24 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 font-mono text-xs"
                value={glossaryText}
                onChange={(e) => setGlossaryText(e.target.value)}
                disabled={running}
                placeholder={"Exile=流亡者\nWraeclast=瓦爾克拉斯"}
              />
            </Fold>
            <Fold title="辨識取代表" meta={replaceCount > 0 ? `${replaceCount} 條` : "未設定"}>
              <span className="text-xs text-neutral-500">每行 錯字=正字</span>
              <textarea
                className="h-20 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 font-mono text-xs"
                value={replaceText}
                onChange={(e) => setReplaceText(e.target.value)}
                disabled={running}
                placeholder={"(辨識常錯的人名、專有名詞)\n騎士=棋士\n錯字=(留空代表刪除)"}
              />
            </Fold>
            <Fold title="網頁翻譯" last>
              <WebTranslateCard models={info?.models ?? []} />
            </Fold>
          </div>

          <section className="mt-auto flex flex-col gap-1 px-1 text-xs text-neutral-500">
            <div>輸出裝置:{info?.device ?? "—"}</div>
            <div>GPU:{info?.gpu ? `${info.gpu[0]},已用 ${info.gpu_used_mib ?? "?"} / ${info.gpu[1]} MiB` : "—"}</div>
            <div className="truncate" title={info?.data_dir}>
              資料:{info?.data_dir}
            </div>
          </section>
        </aside>

        {/* 右:紀錄與指標 */}
        <main className="flex-1 flex flex-col min-w-0">
          <div className="px-5 py-3.5 border-b border-neutral-800 grid grid-cols-5 gap-2 text-xs">
            <Metric label="句數" value={stats.n} />
            <Metric label="辨識 p50" value={stats.asrP50} unit="ms" />
            <Metric label="譯文首字 p50" value={stats.mtFirstP50} unit="ms" />
            <Metric label="端到端首字 p50" value={stats.e2eP50} unit="ms" warn={(stats.e2eP50 ?? 0) > 2000} />
            <Metric label="端到端首字 p95" value={stats.e2eP95} unit="ms" warn={(stats.e2eP95 ?? 0) > 2000} />
          </div>
          <PerfPanel running={running} />
          <div className="px-5 pt-3 pb-2 flex items-center gap-2">
            <h2 className="text-[15px] font-bold">辨識紀錄</h2>
            <span className="text-xs text-neutral-500">{sentences.length} 句</span>
            <button
              className="ml-auto min-h-8 rounded-md border border-neutral-700 bg-neutral-800 px-3 text-[13px] hover:bg-neutral-700 disabled:opacity-40 disabled:hover:bg-neutral-800"
              onClick={exportTranscript}
              disabled={sentences.length === 0}
              title="匯出為純文字對照檔(時間、說話者、原文、譯文)"
            >
              匯出紀錄
            </button>
          </div>
          <div ref={listRef} className="flex-1 overflow-y-auto px-5 pt-1 pb-5 flex flex-col gap-2">
            {sentences.length === 0 && (
              <div className="text-neutral-500 mt-8 text-center">
                按「開始」後播放英文、日文或韓文影片,字幕會出現在這裡與懸浮視窗。
              </div>
            )}
            {sentences.map((s) => (
              <SentenceRow key={s.id} s={s} speakers={speakers} />
            ))}
            {partial && (
              <div className="rounded-[10px] border border-dashed border-neutral-700 px-3.5 py-3 text-neutral-400 italic">
                講話中:{partialSpeaker && <span className={`not-italic ${speakerColor(partialSpeaker, speakers)}`}>{partialSpeaker} </span>}
                {partial}
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

function GoSettings({ what, onClick }: { what: string; onClick: () => void }) {
  return (
    <button className="text-left text-amber-400 text-xs hover:underline" data-role="go-settings" onClick={onClick}>
      {what},到「設定」下載 →
    </button>
  );
}

function Metric({ label, value, unit, warn }: { label: string; value: number | null | undefined; unit?: string; warn?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 rounded-lg bg-neutral-900/80 px-3 py-2">
      <div className="text-neutral-500">{label}</div>
      <div className={`text-lg font-bold tabular-nums ${warn ? "text-amber-400" : "text-neutral-100"}`}>
        {value == null ? "—" : value}
        {value != null && unit ? <span className="text-xs font-normal text-neutral-500"> {unit}</span> : null}
      </div>
    </div>
  );
}

/** 左欄的一張卡片(可帶標題與右側小字)。 */
function Card({ title, hint, children }: { title?: string; hint?: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2.5 rounded-xl border border-neutral-800 bg-neutral-900/60 px-4 py-3.5">
      {title && (
        <div className="flex items-baseline gap-2" data-card={title}>
          <h2 className="text-[13px] font-bold text-neutral-300">{title}</h2>
          {hint && <span className="truncate text-[11px] text-neutral-500" title={hint}>{hint}</span>}
        </div>
      )}
      {children}
    </section>
  );
}

/** 分段選擇鈕(來源語言、斷句、字幕模式)。 */
function Segmented({
  options,
  value,
  onChange,
  disabled,
}: {
  options: { value: string; label: string; title?: string }[];
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="grid gap-1 rounded-lg bg-neutral-950 p-[3px]" style={{ gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))` }}>
      {options.map((o) => (
        <button
          key={o.value}
          title={o.title}
          disabled={disabled}
          aria-pressed={value === o.value}
          onClick={() => onChange(o.value)}
          className={`min-h-8 rounded-md text-[13px] disabled:opacity-50 ${value === o.value ? "bg-sky-950 text-white" : "text-neutral-400 hover:text-neutral-200"}`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** 可展開的一列:標題、目前狀態小字,點開才看到內容(內容一直在頁面上)。 */
function Fold({ title, meta, last, children }: { title: string; meta?: string; last?: boolean; children: React.ReactNode }) {
  return (
    <details className={`group ${last ? "" : "border-b border-neutral-800"}`}>
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-2 px-4 hover:bg-neutral-800/40 [&::-webkit-details-marker]:hidden">
        <span>{title}</span>
        {meta && <span className="text-xs text-neutral-500">{meta}</span>}
        <span className="ml-auto text-neutral-500 transition-transform group-open:rotate-90">›</span>
      </summary>
      <div className="flex flex-col gap-2 px-4 pb-3.5 pt-1 text-[13px]">{children}</div>
    </details>
  );
}

function SentenceRow({ s, speakers }: { s: Sentence; speakers: string[] }) {
  const t = s.timing;
  return (
    <article className="flex flex-col gap-1 rounded-[10px] bg-neutral-900/80 px-3.5 py-3" data-sentence-id={s.id} data-lang={s.lang}>
      <div className="flex flex-wrap items-center gap-2 text-xs text-neutral-500">
        {s.speaker && (
          <span className={`rounded-full bg-neutral-800 px-2 py-px font-bold ${speakerColor(s.speaker, speakers)}`}>{s.speaker}</span>
        )}
        <span className="uppercase">{s.lang}</span>
        {s.revised && (
          <span
            className="cursor-help rounded bg-amber-950 px-1.5 text-amber-300"
            title={`回修前\n原文:${s.prevSource ?? s.source}\n快譯:${s.prevTranslation ?? "(無)"}`}
          >
            修
          </span>
        )}
        {s.recovered && (
          <span className="cursor-help rounded bg-sky-950 px-1.5 text-sky-300" title="回修補回:即時切段漏掉的語音">
            補
          </span>
        )}
        {s.asrMs > 0 && <span>辨識 {s.asrMs} ms</span>}
        {t && (
          <span>
            · 譯文首字 {t.mt_first_ms ?? "—"} ms · 端到端 {t.e2e_first_ms ?? "—"} ms
          </span>
        )}
        {s.state === "failed" && <span className="text-amber-400">翻譯失敗:{s.error}</span>}
        {s.state === "no_translate" && <span>不翻譯</span>}
        <span className="ml-auto">#{s.id}</span>
      </div>
      <div className="text-neutral-300" data-role="source">
        {s.source}
      </div>
      {s.state !== "no_translate" && (
        <div className={`text-base font-medium ${s.state === "translating" ? "text-neutral-400" : "text-white"}`}>
          {s.translation || (s.state === "translating" ? "…" : "")}
        </div>
      )}
    </article>
  );
}
