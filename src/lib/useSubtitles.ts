import { useEffect, useReducer } from "react";
import { listen } from "@tauri-apps/api/event";
import type { PipelineEvent, Sentence } from "./types";

interface State {
  sentences: Sentence[];
  status: string;
  errors: string[];
  /** 講話中的即時預覽原文(灰字);空字串 = 無 */
  partial: string;
  /** 預覽階段暫定的說話者 */
  partialSpeaker: string | null;
  /** 已知說話者(依出現順序) */
  speakers: string[];
  /** 目前斷句參數 */
  segmentation: { min_silence_ms: number; max_speech_ms: number; reason: string } | null;
}

const MAX_SENTENCES = 500;

type Action = PipelineEvent | { type: "reset" };

function reducer(state: State, ev: Action): State {
  switch (ev.type) {
    case "reset":
      return { sentences: [], status: state.status, errors: [], partial: "", partialSpeaker: null, speakers: [], segmentation: null };
    case "segmentation":
      return { ...state, segmentation: { min_silence_ms: ev.min_silence_ms, max_speech_ms: ev.max_speech_ms, reason: ev.reason } };
    case "speaker_merged":
      return {
        ...state,
        speakers: state.speakers.filter((n) => n !== ev.from),
        sentences: state.sentences.map((s) => (s.speaker === ev.from ? { ...s, speaker: ev.into } : s)),
      };
    case "speakers": {
      // 改名時把舊名換成新名(舊名不在新清單、新名是新出現的)
      const gone = state.speakers.filter((n) => !ev.names.includes(n));
      const added = ev.names.filter((n) => !state.speakers.includes(n));
      let sentences = state.sentences;
      if (gone.length === 1 && added.length === 1) {
        sentences = sentences.map((s) => (s.speaker === gone[0] ? { ...s, speaker: added[0] } : s));
      } else if (gone.length === 1 && added.length === 0) {
        // 合併到既有名字:無法得知目標,保留舊標籤
      }
      return { ...state, speakers: ev.names, sentences };
    }
    case "partial": {
      // 重辨識偶爾會短暫吐出比上一次短很多的結果(抖動),這種就不更新,減少閃動
      if (ev.text !== "" && ev.text.length < state.partial.length * 0.6) return state;
      return { ...state, partial: ev.text, partialSpeaker: ev.text ? ev.speaker ?? state.partialSpeaker : null };
    }
    case "status":
      return { ...state, status: ev.message };
    case "error":
      return { ...state, errors: [...state.errors.slice(-19), ev.message] };
    case "segment":
      return state;
    case "transcript_merged":
      // from 被併入 into:移除 from,更新 into 的原文(仍在等待翻譯)
      return {
        ...state,
        sentences: state.sentences
          .filter((s) => s.id !== ev.from)
          .map((s) => (s.id === ev.into ? { ...s, source: ev.text, state: "translating", startMs: ev.start_ms, endMs: ev.end_ms } : s)),
      };
    case "source_revised":
      return {
        ...state,
        sentences: state.sentences.map((s) =>
          s.id === ev.id ? { ...s, prevSource: s.prevSource ?? s.source, source: ev.text, revised: true } : s,
        ),
      };
    case "translation_revised":
      // 找不到 id 就不動(與 translation_done 不同,回修不補句)
      return {
        ...state,
        sentences: state.sentences.map((s) =>
          s.id === ev.id
            ? { ...s, prevTranslation: s.prevTranslation ?? (s.translation || undefined), translation: ev.text, state: "done", revised: true }
            : s,
        ),
      };
    case "recovered": {
      if (state.sentences.some((s) => s.id === ev.id)) return state;
      const s: Sentence = {
        id: ev.id,
        source: ev.text,
        lang: ev.lang,
        speaker: ev.speaker,
        translation: "",
        state: ev.will_translate ? "translating" : "no_translate",
        asrMs: 0,
        at: Date.now(),
        startMs: ev.start_ms,
        endMs: ev.end_ms,
        recovered: true,
      };
      let idx: number;
      if (ev.after === null) {
        const first = state.sentences.findIndex((x) => (x.startMs ?? Infinity) >= ev.start_ms);
        idx = first < 0 ? state.sentences.length : first;
      } else {
        const a = state.sentences.findIndex((x) => x.id === ev.after);
        idx = a < 0 ? state.sentences.length : a + 1;
      }
      const sentences = [...state.sentences.slice(0, idx), s, ...state.sentences.slice(idx)].slice(-MAX_SENTENCES);
      return { ...state, sentences };
    }
    case "transcript": {
      state = { ...state, partial: "", partialSpeaker: null };
      // 合併(短句等待)會讓同一 id 再出現;以 id 覆蓋。
      const existing = state.sentences.findIndex((s) => s.id === ev.id);
      const s: Sentence = {
        id: ev.id,
        source: ev.text,
        lang: ev.lang,
        speaker: ev.speaker,
        translation: "",
        state: ev.will_translate ? "translating" : "no_translate",
        asrMs: ev.asr_ms,
        at: Date.now(),
        startMs: ev.start_ms,
        endMs: ev.end_ms,
      };
      const sentences =
        existing >= 0
          ? state.sentences.map((x, i) => (i === existing ? s : x))
          : [...state.sentences, s].slice(-MAX_SENTENCES);
      return { ...state, sentences };
    }
    case "translation_delta":
      return {
        ...state,
        sentences: state.sentences.map((s) =>
          s.id === ev.id ? { ...s, translation: s.translation + ev.delta, state: "translating" } : s,
        ),
      };
    case "translation_done": {
      const idx = state.sentences.findIndex((s) => s.id === ev.id);
      if (idx < 0) {
        // 合併後 id 可能是較早那句;補一筆
        const s: Sentence = {
          id: ev.id,
          source: ev.source,
          lang: "unknown",
          speaker: ev.speaker,
          translation: ev.text,
          state: "done",
          asrMs: ev.timing.asr_ms ?? 0,
          timing: ev.timing,
          at: Date.now(),
        };
        return { ...state, sentences: [...state.sentences, s].slice(-MAX_SENTENCES) };
      }
      return {
        ...state,
        sentences: state.sentences.map((s) =>
          s.id === ev.id
            ? { ...s, source: ev.source, translation: ev.text, state: "done", timing: ev.timing, speaker: ev.speaker ?? s.speaker }
            : s,
        ),
      };
    }
    case "translation_failed":
      return {
        ...state,
        sentences: state.sentences.map((s) =>
          s.id === ev.id ? { ...s, state: "failed", error: ev.error } : s,
        ),
      };
  }
}

/** 字幕事件來源:主視窗與懸浮字幕走 Tauri 事件,OBS 頁面走 `/events`(SSE)。 */
export interface SubtitleBus {
  onPipeline(cb: (ev: PipelineEvent) => void): () => void;
  /** 新回合(清除上一輪) */
  onReset(cb: () => void): () => void;
  /** 後端判定閒置(沒聲音且字幕沒新內容) */
  onIdle(cb: () => void): () => void;
}

const tauriListen = <T,>(name: string, cb: (p: T) => void) => {
  const un = listen<T>(name, (e) => cb(e.payload));
  return () => void un.then((f) => f());
};

export const tauriBus: SubtitleBus = {
  onPipeline: (cb) => tauriListen<PipelineEvent>("pipeline", cb),
  // 後端每次「開始」都會發 session-start:清掉上一輪的句子,避免新舊編號互相覆蓋
  onReset: (cb) => tauriListen("session-start", () => cb()),
  onIdle: (cb) => tauriListen("subtitle-idle", () => cb()),
};

/** OBS 頁面:一條 EventSource 分給各訂閱者;連上時後端先送 reset 與本回合事件,斷線由瀏覽器自動重連。 */
export function sseBus(url: string): SubtitleBus {
  const subs = { pipeline: new Set<(ev: PipelineEvent) => void>(), reset: new Set<() => void>(), idle: new Set<() => void>() };
  const es = new EventSource(url);
  es.onmessage = (m) => {
    let ev: { type: string };
    try {
      ev = JSON.parse(m.data);
    } catch {
      return;
    }
    if (ev.type === "reset") subs.reset.forEach((f) => f());
    else if (ev.type === "idle") subs.idle.forEach((f) => f());
    else subs.pipeline.forEach((f) => f(ev as PipelineEvent));
  };
  const add = <T,>(set: Set<T>, f: T) => {
    set.add(f);
    return () => void set.delete(f);
  };
  return {
    onPipeline: (cb) => add(subs.pipeline, cb),
    onReset: (cb) => add(subs.reset, cb),
    onIdle: (cb) => add(subs.idle, cb),
  };
}

export function useSubtitles(bus: SubtitleBus = tauriBus) {
  const [state, dispatch] = useReducer(reducer, {
    sentences: [],
    status: "",
    errors: [],
    partial: "",
    partialSpeaker: null,
    speakers: [],
    segmentation: null,
  });
  useEffect(() => {
    const a = bus.onPipeline((ev) => dispatch(ev));
    const b = bus.onReset(() => dispatch({ type: "reset" }));
    return () => {
      a();
      b();
    };
  }, [bus]);
  return state;
}
