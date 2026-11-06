import { useEffect, useMemo, useRef } from "react";
import Overlay from "./Overlay";
import { sseBus, useSubtitles, type SubtitleBus } from "./lib/useSubtitles";
import { speakerColor } from "./lib/speakerColor";
import type { OverlayMode } from "./lib/types";

/** OBS 瀏覽器來源頁:`/history` 辨識紀錄、`/subtitle` 字幕。參數走網址:n(紀錄句數)、mode、size(字級 px)。 */
const params = new URLSearchParams(location.search);
const num = (key: string, def: number, min: number, max: number) => {
  const v = Number.parseInt(params.get(key) ?? "", 10);
  return Number.isFinite(v) ? Math.min(max, Math.max(min, v)) : def;
};
const bus = sseBus("/events");
const mode: OverlayMode = params.get("mode") === "bilingual" ? "bilingual" : "translation";

export default function Obs() {
  if (location.pathname === "/subtitle") {
    return <Overlay bus={bus} obs={{ mode, size: num("size", 32, 12, 96) }} />;
  }
  return <History bus={bus} count={num("n", 6, 1, 50)} size={num("size", 26, 12, 96)} />;
}

/** 最近 N 句原文+譯文,新句在下、舊句往上捲走。 */
function History({ bus, count, size }: { bus: SubtitleBus; count: number; size: number }) {
  const { sentences, partial, partialSpeaker, speakers } = useSubtitles(bus);
  const shown = useMemo(() => sentences.slice(-count), [sentences, count]);
  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [shown, partial]);
  const small = Math.round(size * 0.62);
  return (
    <div className="h-screen w-screen overflow-hidden flex flex-col justify-end gap-2 px-4 pb-3" data-role="obs-history">
      {shown.map((s) => {
        const showSpeaker = speakers.length > 1 && !!s.speaker;
        const translated = s.state !== "no_translate";
        return (
          <div key={s.id} className="fade-in-up" data-sentence-id={s.id}>
            <div className="subtitle-text text-neutral-300 leading-snug" style={{ fontSize: small }} data-role="source">
              {showSpeaker && <span className={speakerColor(s.speaker, speakers)}>{s.speaker}:</span>}
              {s.source}
            </div>
            {translated && (
              <div
                className={`subtitle-text font-semibold leading-tight ${s.translation ? "text-white" : "text-neutral-400"}`}
                style={{ fontSize: size }}
                data-role="translation"
              >
                {s.translation || "…"}
              </div>
            )}
          </div>
        );
      })}
      {partial && (
        <div className="subtitle-text italic text-neutral-400" style={{ fontSize: small }}>
          {partialSpeaker && speakers.length > 1 && (
            <span className={`not-italic ${speakerColor(partialSpeaker, speakers)}`}>{partialSpeaker}:</span>
          )}
          {partial}
        </div>
      )}
      <div ref={endRef} />
    </div>
  );
}
