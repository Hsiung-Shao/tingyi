import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useSubtitles } from "./lib/useSubtitles";
import type { OverlayMode } from "./lib/types";

// 使用者回饋(2026-10-06):同時只顯示一句譯文與灰字預覽,畫面最乾淨。
const SHOW_COUNT = 1;

export default function Overlay() {
  const { sentences, partial } = useSubtitles();
  const [mode, setMode] = useState<OverlayMode>("translation");
  const [interactive, setInteractive] = useState(false);

  useEffect(() => {
    const a = listen<string>("overlay-mode", (e) => setMode(e.payload as OverlayMode));
    const b = listen<boolean>("overlay-interactive", (e) => setInteractive(e.payload));
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, []);

  // 只顯示最近一句(優先顯示已有譯文的那句:新句還在等譯文時,保留上一句譯文不要閃成空白)。
  const visible = useMemo(() => {
    const recent = sentences.slice(-SHOW_COUNT - 1);
    if (recent.length > SHOW_COUNT) {
      const newest = recent[recent.length - 1];
      const hasText = newest.translation.length > 0 || newest.state === "no_translate";
      return hasText ? [newest] : [recent[recent.length - 2]];
    }
    return recent;
  }, [sentences]);

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
      {visible.map((s) => {
        const hasTranslation = s.translation.length > 0;
        const mainText =
          s.state === "no_translate" ? s.source : hasTranslation ? s.translation : s.source;
        const pending = s.state === "translating" && !hasTranslation;
        return (
          <div key={s.id} className="fade-in-up text-center max-w-full">
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
        {partial || " "}
      </div>
    </div>
  );
}
