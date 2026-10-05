// 與 crates/tingyi-core/src/pipeline.rs 的 PipelineEvent 對應(serde tag = "type", snake_case)。

export type Lang = "en" | "ja" | "ko" | "zh" | "unknown";

export interface TimingSnapshot {
  id: number;
  audio_ms: number;
  asr_ms: number | null;
  mt_first_ms: number | null;
  mt_total_ms: number | null;
  e2e_first_ms: number | null;
  e2e_done_ms: number | null;
}

export type PipelineEvent =
  | { type: "status"; message: string }
  | { type: "segment"; id: number; audio_ms: number; rms_dbfs: number; wav: string | null }
  | { type: "transcript_merged"; into: number; from: number; text: string }
  | { type: "partial"; text: string; lang: Lang }
  | {
      type: "transcript";
      id: number;
      text: string;
      lang: Lang;
      asr_ms: number;
      will_translate: boolean;
    }
  | { type: "translation_delta"; id: number; delta: string }
  | {
      type: "translation_done";
      id: number;
      source: string;
      text: string;
      timing: TimingSnapshot;
    }
  | { type: "translation_failed"; id: number; error: string }
  | { type: "error"; message: string };

export interface Sentence {
  id: number;
  source: string;
  lang: Lang;
  translation: string;
  state: "transcribed" | "translating" | "done" | "failed" | "no_translate";
  asrMs: number;
  timing?: TimingSnapshot;
  error?: string;
  at: number; // Date.now()
}

export interface ModelInfo {
  name: string;
  note: string;
  present: boolean;
  size_mb: number;
}

export interface Info {
  data_dir: string;
  device: string | null;
  gpu: [string, number] | null;
  gpu_used_mib: number | null;
  models: ModelInfo[];
  sensevoice_ready: boolean;
  vad_ready: boolean;
  llama_server_ready: boolean;
  running: boolean;
  autostart: boolean;
  /// 依顯示記憶體與已下載檔案建議的翻譯模型
  recommended_model: string;
  asr_engines: { id: string; label: string; present: boolean; note: string }[];
}

export type OverlayMode = "bilingual" | "translation";
