import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useSubtitles } from "./lib/useSubtitles";
import type { OverlayMode } from "./lib/types";

const SHOW_COUNT = 2;
const MIN_STAY_MS = 3000;

export default function Overlay() {
  const { sentences, partial } = useSubtitles();
  const [mode, setMode] = useState<OverlayMode>("bilingual");
  const [interactive, setInteractive] = useState(false);
  const [, tick] = useState(0);

  useEffect(() => {
    const a = listen<string>("overlay-mode", (e) => setMode(e.payload as OverlayMode));
    const b = listen<boolean>("overlay-interactive", (e) => setInteractive(e.payload));
    const t = setInterval(() => tick((x) => x + 1), 500);
    return () => {
      a.then((f) => f());
      b.then((f) => f());
      clearInterval(t);
    };
  }, []);

  // 顯示最近 2 句;最新一句未滿 3 秒時不把更早的擠掉(保留 3 句上限避免跳動)。
  const visible = useMemo(() => {
    const now = Date.now();
    const recent = sentences.slice(-SHOW_COUNT - 1);
    if (recent.length > SHOW_COUNT) {
      const newest = recent[recent.length - 1];
      if (now - newest.at < MIN_STAY_MS) return recent;
      return recent.slice(-SHOW_COUNT);
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
      {partial && (
        <div className="subtitle-text text-[22px] leading-tight text-neutral-400/90 italic text-center max-w-full truncate">
          {partial}
        </div>
      )}
    </div>
  );
}
