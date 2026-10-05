import { useEffect, useReducer } from "react";
import { listen } from "@tauri-apps/api/event";
import type { PipelineEvent, Sentence } from "./types";

interface State {
  sentences: Sentence[];
  status: string;
  errors: string[];
  /** 講話中的即時預覽原文(灰字);空字串 = 無 */
  partial: string;
}

const MAX_SENTENCES = 500;

type Action = PipelineEvent | { type: "reset" };

function reducer(state: State, ev: Action): State {
  switch (ev.type) {
    case "reset":
      return { sentences: [], status: state.status, errors: [], partial: "" };
    case "partial": {
      // 重辨識偶爾會短暫吐出比上一次短很多的結果(抖動),這種就不更新,減少閃動
      if (ev.text !== "" && ev.text.length < state.partial.length * 0.6) return state;
      return { ...state, partial: ev.text };
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
          .map((s) => (s.id === ev.into ? { ...s, source: ev.text, state: "translating" } : s)),
      };
    case "transcript": {
      state = { ...state, partial: "" };
      // 合併(短句等待)會讓同一 id 再出現;以 id 覆蓋。
      const existing = state.sentences.findIndex((s) => s.id === ev.id);
      const s: Sentence = {
        id: ev.id,
        source: ev.text,
        lang: ev.lang,
        translation: "",
        state: ev.will_translate ? "translating" : "no_translate",
        asrMs: ev.asr_ms,
        at: Date.now(),
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
            ? { ...s, source: ev.source, translation: ev.text, state: "done", timing: ev.timing }
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

export function useSubtitles() {
  const [state, dispatch] = useReducer(reducer, { sentences: [], status: "", errors: [], partial: "" });
  useEffect(() => {
    const un = listen<PipelineEvent>("pipeline", (e) => dispatch(e.payload));
    // 後端每次「開始」都會發 session-start:清掉上一輪的句子,避免新舊編號互相覆蓋
    const un2 = listen("session-start", () => dispatch({ type: "reset" }));
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, []);
  return state;
}
