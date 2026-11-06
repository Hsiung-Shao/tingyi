import { memo, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Badge, ModelCard, PackageRow, usePackages, type PackageCommand, type PackageView } from "./Packages";
import type { Info, ObsStatus, Sentence } from "./lib/types";
import { AboutSection, type Updater } from "./Updater";

export type Section = "translate" | "recog" | "live" | "about";

interface Props {
  info: Info | null;
  running: boolean;
  model: string;
  onModel: (name: string) => void;
  asrEngine: string;
  onAsrEngine: (id: string) => void;
  onChanged: () => void;
  /** 直播區預覽用:目前的字幕紀錄 */
  sentences: Sentence[];
  /** 目前顯示哪一類(App 持有:標題列「有新版」要能直接跳到「關於與更新」) */
  section: Section;
  onSection: (s: Section) => void;
  updater: Updater;
}

/** 「設定」分頁:左側分類導覽,右側該類的卡片。選用等同字幕頁的下拉選單,下次按「開始」時載入。
 *  三類都保持掛載(只藏起來),切換時下載進度與完成通知不會中斷。 */
export default function SettingsPage(props: Props) {
  const { info, running, model, onModel, asrEngine, onAsrEngine, onChanged, sentences, section, onSection: setSection, updater } = props;
  // 直播預覽只要最後兩句;設定頁常駐掛載,其他區塊不跟著每個譯文 token 重繪(下面兩區用 memo)
  const recent = useMemo(() => sentences.filter((s) => !s.recovered).slice(-2), [sentences]);
  const [obs, setObs] = useState<ObsStatus | null>(null);
  const translate = usePackages("translate_packages_status", undefined, running, onChanged);
  const base = usePackages("base_packages_status", undefined, running, onChanged);

  const mtModels = translate.views.filter((v) => v.m.gguf);
  const engines = info?.asr_engines ?? [];
  const nav: { key: Section; label: string; meta: string; metaOn?: boolean }[] = [
    { key: "translate", label: "翻譯模型", meta: `${mtModels.filter((v) => v.m.ready).length} / ${mtModels.length}` },
    { key: "recog", label: "辨識", meta: `${engines.filter((e) => e.present).length} / ${engines.length}` },
    { key: "live", label: "直播(OBS)", meta: obs?.enabled ? "開啟" : "關閉", metaOn: obs?.enabled },
    { key: "about", label: "關於與更新", meta: updater.info ? "有新版" : updater.current, metaOn: !!updater.info },
  ];

  return (
    <div className="flex flex-1 min-h-0 flex-wrap" data-role="settings-page">
      <aside className="flex w-[220px] shrink-0 flex-col gap-1 border-r border-neutral-800 px-3 py-5">
        {nav.map((n) => (
          <button
            key={n.key}
            data-nav={n.key}
            className={`flex items-center justify-between rounded-lg px-3 py-2.5 text-left ${
              section === n.key ? "bg-sky-950 font-medium text-white" : "text-neutral-300 hover:bg-neutral-900"
            }`}
            onClick={() => setSection(n.key)}
          >
            {n.label}
            <span className={`text-xs ${n.metaOn ? "text-emerald-300" : "text-neutral-500"}`}>{n.meta}</span>
          </button>
        ))}
        <p className="mt-auto px-3 text-xs leading-relaxed text-neutral-500">
          選好後回「字幕」按「開始」才會載入。{running && "執行中不能換模型。"}
        </p>
      </aside>
      <main className="min-w-0 flex-[999_1_560px] overflow-y-auto px-8 py-7">
        <div className={section === "translate" ? "" : "hidden"}>
          <TranslateSection info={info} running={running} model={model} onModel={onModel} views={translate.views} />
        </div>
        <div className={section === "recog" ? "" : "hidden"}>
          <RecogSection info={info} running={running} asrEngine={asrEngine} onAsrEngine={onAsrEngine} onChanged={onChanged} base={base.views} />
        </div>
        <div className={section === "live" ? "" : "hidden"}>
          <LiveSection recent={recent} onStatus={setObs} />
        </div>
        <div className={section === "about" ? "" : "hidden"}>
          <AboutSection u={updater} running={running} />
        </div>
        {(translate.finishedWhileRunning || base.finishedWhileRunning) && (
          <p className="mt-4 text-xs text-amber-400">下載完成,下次按「開始」時生效。</p>
        )}
      </main>
    </div>
  );
}

function SectionHead({ title, desc }: { title: string; desc: string }) {
  return (
    <div className="flex flex-col gap-1.5">
      <h1 className="text-[22px] font-bold">{title}</h1>
      <p className="text-neutral-400">{desc}</p>
    </div>
  );
}

const GROUPS: { title: string; hint?: string; warn?: boolean; match: (name: string) => boolean }[] = [
  { title: "通用(可譯成多種語言)", hint: "Hy-MT2 能譯成中、英、日、韓", match: (n) => n.startsWith("Hy-MT2") },
  { title: "日文特化", hint: "CC BY-NC-SA 4.0,禁止商用;只能譯日文", warn: true, match: (n) => n.startsWith("Sakura") },
  { title: "其他多語翻譯模型", hint: "只能譯成繁體中文", match: (n) => !n.startsWith("Hy-MT2") && !n.startsWith("Sakura") },
];

const TranslateSection = memo(function TranslateSection({
  info,
  running,
  model,
  onModel,
  views,
}: Pick<Props, "info" | "running" | "model" | "onModel"> & { views: PackageView[] }) {
  const server = views.find((v) => !v.m.gguf);
  const note = (name: string) => info?.models.find((m) => m.name === name)?.note;
  return (
    <section data-section="translate" className="flex flex-col gap-6">
      <SectionHead title="翻譯模型" desc="翻譯伺服器必須下載一次;下面的模型擇一使用,依顯示記憶體挑選。" />
      {server && (
        <div className="rounded-xl border border-neutral-800 bg-neutral-900/60 px-5 py-4">
          <PackageRow v={server} sub={`${server.m.license} · 翻譯伺服器(必要)`} scriptHint="" />
        </div>
      )}
      {GROUPS.map((g) => {
        const list = views.filter((v) => v.m.gguf && g.match(v.m.gguf));
        if (list.length === 0) return null;
        return (
          <div key={g.title} className="flex flex-col gap-3">
            <div className="flex items-baseline gap-2">
              <h2 className="text-[15px] font-bold">{g.title}</h2>
              {g.hint && <span className={`text-xs ${g.warn ? "text-amber-400" : "text-neutral-500"}`}>{g.hint}</span>}
            </div>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3">
              {list.map((v) => (
                <ModelCard
                  key={v.m.id}
                  v={v}
                  note={note(v.m.gguf!)}
                  isActive={v.m.gguf === model}
                  running={running}
                  onUse={() => onModel(v.m.gguf!)}
                />
              ))}
            </div>
          </div>
        );
      })}
    </section>
  );
});

const RecogSection = memo(function RecogSection({
  info,
  running,
  asrEngine,
  onAsrEngine,
  onChanged,
  base,
}: Pick<Props, "info" | "running" | "asrEngine" | "onAsrEngine" | "onChanged"> & { base: PackageView[] }) {
  return (
    <section data-section="recog" className="flex flex-col gap-6">
      <SectionHead title="辨識" desc="基本模型每台電腦都要;辨識引擎選一個,需要的檔案在卡片裡下載。" />
      <div className="flex flex-col gap-3 rounded-xl border border-neutral-800 bg-neutral-900/60 px-5 py-4" data-section="base">
        <h2 className="text-[15px] font-bold">基本模型</h2>
        <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
          {base.map((v) => (
            <div key={v.m.id} className="rounded-lg bg-neutral-800/50 p-3">
              <PackageRow v={v} scriptHint="" />
            </div>
          ))}
        </div>
      </div>
      <div className="flex flex-col gap-3" data-section="engines">
        <h2 className="text-[15px] font-bold">辨識引擎</h2>
        <div className="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] items-start gap-3">
          {info?.asr_engines.map((e) =>
            e.id === "sensevoice" ? (
              <EngineCard key={e.id} e={e} views={[]} active={e.id === asrEngine} running={running} onUse={() => onAsrEngine(e.id)} />
            ) : (
              <EngineWithPackages
                key={e.id}
                e={e}
                command={e.id === "routed" ? "routed_models_status" : "engine_packages_status"}
                active={e.id === asrEngine}
                running={running}
                onUse={() => onAsrEngine(e.id)}
                onChanged={onChanged}
              />
            ),
          )}
        </div>
      </div>
    </section>
  );
});

type EngineInfo = Info["asr_engines"][number];

function EngineWithPackages(p: { e: EngineInfo; command: PackageCommand; active: boolean; running: boolean; onUse: () => void; onChanged: () => void }) {
  const { views } = usePackages(p.command, p.command === "engine_packages_status" ? p.e.id : undefined, p.running, p.onChanged);
  return <EngineCard {...p} views={views} />;
}

function EngineCard({ e, views, active, running, onUse }: { e: EngineInfo; views: PackageView[]; active: boolean; running: boolean; onUse: () => void }) {
  const missing = views.filter((v) => !v.m.ready).length;
  const experimental = e.label.includes("實驗");
  const name = e.label.replace(/[((]實驗[^))]*[))]/, "").trim();
  const badge = active ? (
    <Badge tone="active" role="engine-active">
      使用中
    </Badge>
  ) : e.present ? (
    <Badge tone="ready">可用</Badge>
  ) : missing > 0 && missing < views.length ? (
    <Badge tone="muted">缺 {missing} 個檔</Badge>
  ) : (
    <Badge tone="muted">未下載</Badge>
  );
  return (
    <article
      data-engine={e.id}
      className={`flex flex-col gap-2.5 rounded-xl border p-4 ${
        active ? "border-sky-400 bg-sky-950/20" : experimental ? "border-dashed border-neutral-700 bg-neutral-900/60" : "border-neutral-800 bg-neutral-900/60"
      }`}
    >
      <div className="flex items-center gap-2">
        <span className="font-semibold">{name}</span>
        {experimental && <span className="rounded-md bg-amber-950 px-2 py-0.5 text-xs text-amber-300">實驗</span>}
        <span className="ml-auto shrink-0">{badge}</span>
      </div>
      <p className="text-[13px] leading-relaxed text-neutral-400">{e.id === "sensevoice" ? `${e.note};只需要基本模型。` : e.note}</p>
      {views.length > 0 && (
        <div className="flex flex-col gap-2 rounded-lg bg-neutral-800/50 px-3 py-2.5 text-[13px]">
          {views.map((v) => (
            <PackageRow key={v.m.id} v={v} scriptHint={e.id === "routed" ? (v.m.id === "reazonspeech" ? " -WithReazonSpeech" : " -WithParakeet") : undefined} />
          ))}
        </div>
      )}
      {!active && (
        <button
          className="mt-auto min-h-10 rounded-lg border border-neutral-700 bg-neutral-800 px-4 hover:bg-neutral-700 disabled:opacity-40 disabled:hover:bg-neutral-800"
          data-role="engine-use"
          disabled={running || !e.present}
          title={running ? "停止後才能換引擎" : undefined}
          onClick={onUse}
        >
          {e.present ? "使用這個" : "下載完才能使用"}
        </button>
      )}
    </article>
  );
}

const OBS_PREF = "tingyi.obs";

/** 直播用網址:開關、兩個網址卡片(附目前字幕預覽)。開關記在本機,下次開啟自動打開。 */
function LiveSection({ recent, onStatus }: { recent: Sentence[]; onStatus: (s: ObsStatus) => void }) {
  const [st, setSt] = useState<ObsStatus | null>(null);
  const [err, setErr] = useState("");
  const [copied, setCopied] = useState("");
  const apply = (s: ObsStatus) => {
    setSt(s);
    onStatus(s);
  };

  useEffect(() => {
    // 上次有打開就自動再打開;設定頁一直掛著,主視窗開啟時就會跑
    let on = false;
    try {
      on = localStorage.getItem(OBS_PREF) === "1";
    } catch {
      // 讀不到當作沒打開過
    }
    invoke<ObsStatus>(on ? "set_obs_enabled" : "obs_status", on ? { enabled: true } : undefined)
      .then(apply)
      .catch((e) => setErr(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 只在掛載時跑一次
  }, []);

  const toggle = async (enabled: boolean) => {
    setErr("");
    try {
      apply(await invoke<ObsStatus>("set_obs_enabled", { enabled }));
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
  const copy = async (key: string, url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(key);
      setTimeout(() => setCopied(""), 1500);
    } catch {
      setErr("無法複製,請手動選取網址");
    }
  };
  const cards = [
    { key: "history", title: "辨識紀錄", desc: "最近幾句原文+譯文,新句在下方。", url: `${base}/history?n=6&size=26`, hint: "OBS 建議 寬 800 × 高 600" },
    { key: "subtitle", title: "字幕", desc: "最新一句,沒聲音時自動收起。", url: `${base}/subtitle?size=40`, hint: "OBS 建議 寬 1920 × 高 250" },
  ];

  return (
    <section data-section="obs" className="flex max-w-[900px] flex-col gap-6">
      <SectionHead title="直播(OBS 瀏覽器來源)" desc="在 OBS 加「瀏覽器」來源貼上網址,背景是透明的。只有這台電腦連得到。" />
      <label className="flex cursor-pointer items-center gap-3.5 rounded-xl border border-neutral-800 bg-neutral-900/60 px-5 py-4">
        {/* 狀態讀回前停用:否則看起來是關的,按下去送出的是「開」 */}
        <input
          type="checkbox"
          className="h-5 w-5 accent-sky-400"
          checked={!!st?.enabled}
          disabled={!st}
          onChange={(e) => toggle(e.target.checked)}
          data-role="obs-toggle"
        />
        <span className="flex flex-col gap-0.5">
          <span className="font-medium">開啟直播用網址</span>
          <span className="text-xs text-neutral-500">下次開啟聽譯會自動打開</span>
        </span>
        {st?.port && (
          <span className="ml-auto">
            <Badge tone="ready">埠 {st.port}</Badge>
          </span>
        )}
      </label>
      {st?.enabled && (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] gap-3">
          {cards.map((c) => (
            <article key={c.key} className="flex flex-col gap-3 rounded-xl border border-neutral-800 bg-neutral-900/60 p-4">
              <div className="flex flex-col gap-1">
                <span className="font-semibold">{c.title}</span>
                <span className="text-[13px] text-neutral-400">{c.desc}</span>
              </div>
              <div
                className={`flex h-[120px] flex-col justify-end gap-1 overflow-hidden rounded-lg bg-neutral-800 px-3 py-2.5 ${c.key === "subtitle" ? "items-center text-center" : ""}`}
              >
                {recent.length === 0 ? (
                  <span className="self-center text-xs text-neutral-500">開始後這裡會顯示目前的字幕</span>
                ) : c.key === "history" ? (
                  recent.map((s) => (
                    <div key={s.id} className="flex flex-col">
                      <span className="truncate text-[11px] text-neutral-300">{s.source}</span>
                      <span className="truncate text-[15px] font-bold text-white">{s.translation || s.source}</span>
                    </div>
                  ))
                ) : (
                  <span className="block max-w-full truncate text-lg font-bold text-white">{recent[recent.length - 1].translation || recent[recent.length - 1].source}</span>
                )}
              </div>
              <div className="flex gap-2">
                <input
                  readOnly
                  value={c.url}
                  aria-label={`${c.title}網址`}
                  className="min-h-10 min-w-0 flex-1 rounded-lg border border-neutral-700 bg-neutral-950 px-2.5 font-mono text-xs"
                  data-role={`obs-url-${c.key}`}
                  onFocus={(e) => e.target.select()}
                />
                <button className="min-h-10 rounded-lg bg-sky-700 px-4 text-white hover:bg-sky-600" onClick={() => copy(c.key, c.url)}>
                  {copied === c.key ? "已複製" : "複製"}
                </button>
              </div>
              <span className="text-xs text-neutral-500">{c.hint}</span>
            </article>
          ))}
        </div>
      )}
      {st?.enabled && <p className="text-xs leading-relaxed text-neutral-500">網址參數:n = 紀錄句數、size = 字級、mode=bilingual 讓字幕也顯示原文。</p>}
      {err && <p className="text-xs text-rose-400">{err}</p>}
    </section>
  );
}
