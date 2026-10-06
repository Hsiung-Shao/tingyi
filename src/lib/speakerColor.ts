// 說話者配色:主視窗與懸浮字幕共用,依出現順序循環。
const SPEAKER_COLORS = [
  "text-sky-300",
  "text-amber-300",
  "text-emerald-300",
  "text-pink-300",
  "text-violet-300",
  "text-orange-300",
];

export const MULTI_SPEAKER = "多人";

export function speakerColor(name: string | null, speakers: string[]) {
  if (!name) return "text-neutral-400";
  if (name === MULTI_SPEAKER) return "text-neutral-300";
  const i = speakers.indexOf(name);
  return SPEAKER_COLORS[(i < 0 ? 0 : i) % SPEAKER_COLORS.length];
}
