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
  | { type: "transcript_merged"; into: number; from: number; text: string; start_ms: number; end_ms: number }
  | { type: "source_revised"; id: number; text: string }
  | { type: "translation_revised"; id: number; text: string }
  | {
      type: "recovered";
      id: number;
      after: number | null;
      text: string;
      lang: Lang;
      speaker: string | null;
      start_ms: number;
      end_ms: number;
      will_translate: boolean;
    }
  | { type: "partial"; text: string; lang: Lang; speaker: string | null }
  | {
      type: "transcript";
      id: number;
      text: string;
      lang: Lang;
      asr_ms: number;
      will_translate: boolean;
      speaker: string | null;
      start_ms: number;
      end_ms: number;
    }
  | { type: "segmentation"; min_silence_ms: number; max_speech_ms: number; reason: string }
  | { type: "speakers"; names: string[] }
  | { type: "speaker_merged"; from: string; into: string }
  | { type: "translation_delta"; id: number; delta: string }
  | {
      type: "translation_done";
      id: number;
      source: string;
      text: string;
      timing: TimingSnapshot;
      speaker: string | null;
    }
  | { type: "translation_failed"; id: number; error: string }
  | { type: "error"; message: string };

export interface Sentence {
  id: number;
  source: string;
  lang: Lang;
  speaker: string | null;
  translation: string;
  state: "transcribed" | "translating" | "done" | "failed" | "no_translate";
  asrMs: number;
  timing?: TimingSnapshot;
  error?: string;
  at: number; // Date.now()
  /** 在整條串流中的位置(毫秒),回修插入補回句時排序用 */
  startMs?: number;
  endMs?: number;
  /** 回修改過原文或譯文 */
  revised?: boolean;
  /** 回修補回的句子(VAD 漏掉的語音) */
  recovered?: boolean;
  /** 被回修取代前的原文 / 快譯(懸停對照用) */
  prevSource?: string;
  prevTranslation?: string;
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
  speaker_model_ready: boolean;
  seg_offset_ms: number;
  seg_adaptive: boolean;
  seg_current_ms: number;
}

/** 網頁翻譯(Chrome 擴充)的狀態,`web_status` 回傳 */
export interface WebStatus {
  status: "not-downloaded" | "downloading" | "stopped" | "starting" | "ready" | "error";
  message: string | null;
  /** 網頁翻譯設定的模型 */
  web_model: string;
  /** 共用伺服器執行中的模型;沒在跑為 null */
  server_model: string | null;
  /** 字幕正在使用共用伺服器(此時跑的是字幕的模型) */
  subtitles: boolean;
}

/** 主視窗的開始選項;存進 settings.json,背景喚起時也用得到 */
export interface Prefs {
  model: string;
  translate: boolean;
  asr_engine: string;
  lang: string;
  speaker_id: boolean;
  refine: boolean;
  glossary: string;
  replacements: string;
  /** 字幕譯成哪種語言:zh-TW / zh-CN / en / ja / ko */
  target_lang: string;
  /** 簡體譯文怎麼轉(目標繁中時):terms / script / none */
  zh_convert: string;
  /** 遊戲術語包 id;空字串 = 不使用 */
  term_pack?: string;
}

/** 資料目錄裡的遊戲術語包(對應 Rust `terms::PackInfo`)。 */
export interface TermPackInfo {
  id: string;
  label: string;
  game_version: string;
  entries: number;
  aliases: number;
  hotwords: number;
}

export type Sensitivity = "fast" | "normal" | "slow";

export interface GpuSample {
  util_pct: number;
  mem_used_mib: number;
  mem_total_mib: number;
  temp_c: number;
}
export interface ProcSample {
  pid: number;
  name: string;
  cpu_pct: number;
  rss_mb: number;
}
export interface SysSample {
  ts_ms: number;
  cpu_pct: number;
  mem_used_mb: number;
  mem_total_mb: number;
  gpu: GpuSample | null;
  procs: ProcSample[];
}
export interface StatsSnapshot {
  segments: number;
  asr_avg_ms: number;
  asr_last_ms: number;
  preview_count: number;
  preview_avg_ms: number;
  speaker_count: number;
  speaker_avg_ms: number;
  speaker_last_ms: number;
  mt_queue: number;
  mt_done: number;
  mt_first_avg_ms: number;
  mt_total_avg_ms: number;
  mt_last_first_ms: number;
  merges: number;
  audio_backlog: number;
  audio_backlog_max: number;
  speaking: boolean;
  refine_count: number;
  refine_avg_ms: number;
  refine_last_ms: number;
  revised_sources: number;
  recovered: number;
  refine_mt_done: number;
  /** 擷取緩衝溢位(掉資料)次數;正常為 0 */
  capture_discontinuities: number;
  capture_silent_packets: number;
  e2e_p50: number;
  e2e_recent: number[];
}
export interface Metrics {
  system: SysSample | null;
  pipeline: StatsSnapshot | null;
  running: boolean;
}

export type OverlayMode = "bilingual" | "translation";

/** 音訊來源(對應 Rust `AudioSource`,serde tag = kind)。 */
export type AudioSource =
  | { kind: "system" }
  | { kind: "app"; pid: number; exe: string; start_time: number };

/** 音訊來源清單的分組(對應 Rust `SourceGroup`)。 */
export type SourceGroup = "playing" | "windowed" | "background";

/** 音訊來源清單的一項(對應 Rust `SourceEntry`);選它就是擷取 `pid` 的整棵程序樹。 */
export type SourceEntry = {
  group: SourceGroup;
  /** 最上層程序 */
  pid: number;
  exe: string;
  start_time: number;
  active: boolean;
} & ({ kind: "window"; hwnd: number; title: string } | { kind: "background"; name: string });

/** 專用模型的就緒與下載狀態(`routed_models_status`)。 */
export interface RoutedModelState {
  id: string;
  label: string;
  lang: Lang;
  size_bytes: number;
  ready: boolean;
  downloading: boolean;
  /** 已下載位元組:下載中為即時進度;中斷過則是可續傳的起點 */
  done: number;
  license: string;
  license_url: string;
  /** 翻譯模型的名稱(設定頁「使用這個」用);其他套件為 null */
  gguf: string | null;
}

/** 直播用網址(OBS 瀏覽器來源)的狀態。 */
export interface ObsStatus {
  enabled: boolean;
  port: number | null;
}

/** `model-download` 事件。 */
export interface ModelDownloadProgress {
  id: string;
  phase: "downloading" | "verifying" | "done" | "error" | "cancelled";
  done: number;
  total: number;
  bps: number;
  message: string | null;
}

export interface AudioSourceState {
  source: AudioSource;
  /** 指定程式已關閉,等它重新開啟中 */
  waiting: boolean;
}
