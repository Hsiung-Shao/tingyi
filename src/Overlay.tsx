import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { tauriBus, useSubtitles, type SubtitleBus } from "./lib/useSubtitles";
import { speakerColor } from "./lib/speakerColor";
import type { OverlayMode, Sentence } from "./lib/types";

// 使用者回饋(2026-10-06):同時只顯示一句譯文與灰字預覽,畫面最乾淨。
const SHOW_COUNT = 1;

// 只顯示最近一句(優先顯示已有譯文的那句:新句還在等譯文時,保留上一句譯文不要閃成空白)。
function latestShown(list: Sentence[]): Sentence[] {
  const recent = list.slice(-SHOW_COUNT - 1);
  if (recent.length > SHOW_COUNT) {
    const newest = recent[recent.length - 1];
    const hasText = newest.translation.length > 0 || newest.state === "no_translate";
    return hasText ? [newest] : [recent[recent.length - 2]];
  }
  return recent;
}

/** 懸浮字幕;OBS 的 `/subtitle` 頁也用它(`obs` 給定時:事件走 SSE、模式與字級由網址參數決定,不聽主視窗)。 */
export default function Overlay({ bus = tauriBus, obs }: { bus?: SubtitleBus; obs?: { mode: OverlayMode; size: number } }) {
  // 後端判定閒置(沒聲音且字幕沒有新內容)時收起:dismissed 記下當時最新句與預覽,之後只顯示比它新的內容;
  // 收起時還在等譯文的句子(pending),譯文到了也算新內容。新回合時清掉
  const { sentences, partial, partialSpeaker, speakers, dismissed } = useSubtitles(bus);
  const [mode, setMode] = useState<OverlayMode>(obs?.mode ?? "translation");
  const size = obs?.size ?? 32;
  const [interactive, setInteractive] = useState(false);
  // 補回句屬於歷史修正,不插進即時字幕;被回修的當前句則會自動更新
  const live = useMemo(() => sentences.filter((s) => !s.recovered), [sentences]);
  useEffect(() => {
    if (obs) return;
    const a = listen<string>("overlay-mode", (e) => setMode(e.payload as OverlayMode));
    const b = listen<boolean>("overlay-interactive", (e) => setInteractive(e.payload));
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, [obs]);

  const { shown, freshPartial, idle } = useMemo(() => {
    const fresh = dismissed
      ? live.filter((s) => s.id > dismissed.id || (dismissed.pending.has(s.id) && s.translation.length > 0))
      : live;
    const freshPartial = dismissed && partial === dismissed.partial ? "" : partial;
    const idle = dismissed !== null && fresh.length === 0 && freshPartial === "";
    return { shown: latestShown(fresh), freshPartial, idle };
  }, [live, partial, dismissed]);
  // 收起後還沒有新東西:淡出收起前一刻畫面上的內容。不能從全部句子重挑,
  // 否則最新句沒有譯文時會挑到上一次收起前的舊句,讓它閃回來。
  const lastShown = useRef({ visible: [] as Sentence[], partial: "" });
  useEffect(() => {
    if (!idle) lastShown.current = { visible: shown, partial: freshPartial };
  });
  const visible = idle ? lastShown.current.visible : shown;
  const shownPartial = idle ? lastShown.current.partial : freshPartial;
  // 解鎖拖曳時不淡出,讓使用者看得到字幕位置
  const faded = idle && !interactive;

  const startDrag = (e: React.MouseEvent) => {
    if (!interactive || obs) return;
    if (e.button !== 0) return;
    void getCurrentWindow().startDragging();
  };

  return (
    <div
      className={`h-screen w-screen flex flex-col justify-end items-center gap-1 px-6 pb-3 ${
        interactive ? "bg-black/40 outline-2 outline-dashed outline-yellow-400 cursor-move" : ""
      }`}
      onMouseDown={startDrag}
    >
      {interactive && (
        <div className="absolute top-1 left-2 text-xs text-yellow-300 subtitle-text">
          拖曳移動字幕位置;在主視窗按「鎖定」恢復滑鼠穿透
        </div>
      )}
      <div
        data-role="overlay-content"
        data-idle={faded ? "1" : "0"}
        className={`w-full flex flex-col items-center gap-1 transition-opacity duration-500 ${faded ? "opacity-0" : ""}`}
      >
        {visible.map((s) => {
          const hasTranslation = s.translation.length > 0;
          const mainText = s.state === "no_translate" ? s.source : hasTranslation ? s.translation : s.source;
          // 超過一位說話者時在譯文前加有顏色的名字
          const showSpeaker = speakers.length > 1 && !!s.speaker;
          const pending = s.state === "translating" && !hasTranslation;
          return (
            <div key={s.id} data-sentence-id={s.id} className="fade-in-up text-center max-w-full">
              {mode === "bilingual" && s.state !== "no_translate" && hasTranslation && (
                <div
                  className="subtitle-text leading-snug text-neutral-300 truncate max-w-[1000px]"
                  style={{ fontSize: Math.round(size * 0.56) }}
                >
                  {s.source}
                </div>
              )}
              <div
                style={{ fontSize: size }}
                className={`subtitle-text leading-tight font-semibold ${
                  pending ? "text-neutral-400" : "text-white"
                } ${s.state === "failed" ? "text-amber-200" : ""}`}
              >
                {showSpeaker && <span className={speakerColor(s.speaker, speakers)}>{s.speaker}:</span>}
                {mainText}
                {s.state === "translating" && hasTranslation && (
                  <span className="inline-block w-[0.4em] animate-pulse">▍</span>
                )}
              </div>
            </div>
          );
        })}
        {/* 預覽列固定佔一行高度,有無文字都不讓上方字幕跳動 */}
        <div className="subtitle-text h-[28px] text-[22px] leading-[28px] text-neutral-400/90 italic text-center max-w-full truncate">
          {shownPartial && partialSpeaker && speakers.length > 1 && (
            <span className={`not-italic ${speakerColor(partialSpeaker, speakers)}`}>{partialSpeaker}:</span>
          )}
          {shownPartial || " "}
        </div>
      </div>
    </div>
  );
}
