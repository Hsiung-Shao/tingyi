import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useSubtitles } from "./lib/useSubtitles";
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

export default function Overlay() {
  const { sentences, partial, partialSpeaker, speakers } = useSubtitles();
  const [mode, setMode] = useState<OverlayMode>("translation");
  const [interactive, setInteractive] = useState(false);
  // 後端判定閒置(沒聲音且字幕沒有新內容)時收起:記下當時最新句與預覽,之後只顯示比它新的內容
  const [dismissed, setDismissed] = useState<{ id: number; partial: string } | null>(null);
  // 補回句屬於歷史修正,不插進即時字幕;被回修的當前句則會自動更新
  const live = useMemo(() => sentences.filter((s) => !s.recovered), [sentences]);
  const latest = useRef({ id: -1, partial: "" });
  latest.current = { id: live.length > 0 ? live[live.length - 1].id : -1, partial };

  useEffect(() => {
    const a = listen<string>("overlay-mode", (e) => setMode(e.payload as OverlayMode));
    const b = listen<boolean>("overlay-interactive", (e) => setInteractive(e.payload));
    const c = listen("subtitle-idle", () => setDismissed({ ...latest.current }));
    // 新回合句子 id 從頭算,收起標記要清掉
    const d = listen("session-start", () => setDismissed(null));
    return () => {
      a.then((f) => f());
      b.then((f) => f());
      c.then((f) => f());
      d.then((f) => f());
    };
  }, []);

  const { visible, shownPartial, idle } = useMemo(() => {
    const fresh = dismissed ? live.filter((s) => s.id > dismissed.id) : live;
    const freshPartial = dismissed && partial === dismissed.partial ? "" : partial;
    // 收起後還沒有新東西:保留原內容做淡出
    const idle = dismissed !== null && fresh.length === 0 && freshPartial === "";
    return { visible: latestShown(idle ? live : fresh), shownPartial: idle ? partial : freshPartial, idle };
  }, [live, partial, dismissed]);

  const startDrag = (e: React.MouseEvent) => {
    if (!interactive) return;
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
        data-idle={idle ? "1" : "0"}
        className={`w-full flex flex-col items-center gap-1 transition-opacity duration-500 ${idle ? "opacity-0" : ""}`}
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
                <div className="subtitle-text text-[18px] leading-snug text-neutral-300 truncate max-w-[1000px]">
                  {s.source}
                </div>
              )}
              <div
                className={`subtitle-text text-[32px] leading-tight font-semibold ${
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
