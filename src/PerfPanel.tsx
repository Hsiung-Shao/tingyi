import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Metrics } from "./lib/types";

/** 效能監測面板:每 2 s 拉 get_metrics;可收合;可匯出診斷 JSON。 */
export default function PerfPanel({ running }: { running: boolean }) {
  const [open, setOpen] = useState(false);
  const [m, setM] = useState<Metrics | null>(null);
  const [exportMsg, setExportMsg] = useState("");

  useEffect(() => {
    if (!open) return;
    let alive = true;
    const tick = () =>
      invoke<Metrics>("get_metrics")
        .then((x) => alive && setM(x))
        // 取樣器尚未就緒或後端忙碌時略過這一拍,下一拍再更新
        .catch(() => {});
    tick();
    const t = setInterval(tick, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [open, running]);

  const exportDiag = async () => {
    try {
      const p = await invoke<string>("export_diagnostics");
      setExportMsg(`已匯出:${p}`);
    } catch (e) {
      setExportMsg(`匯出失敗:${String(e)}`);
    }
  };

  const sys = m?.system ?? null;
  const p = m?.pipeline ?? null;
  const proc = (name: string) => sys?.procs.find((x) => x.name === name);

  return (
    <div className="border-b border-neutral-800">
      <button
        className="w-full text-left px-4 py-1.5 text-xs text-neutral-400 hover:text-neutral-200 flex items-center gap-2"
        onClick={() => setOpen((o) => !o)}
      >
        <span>{open ? "▾" : "▸"}</span>
        <span>效能監測</span>
        {!open && sys && (
          <span className="text-neutral-500">
            CPU {sys.cpu_pct.toFixed(0)}% · GPU {sys.gpu ? `${sys.gpu.util_pct}% / ${sys.gpu.mem_used_mib} MiB` : "—"}
            {p ? ` · 端到端 p50 ${p.e2e_p50} ms` : ""}
          </span>
        )}
      </button>
      {open && (
        <div className="px-4 pb-3 flex flex-col gap-2 text-xs">
          <div className="grid grid-cols-4 gap-2">
            <Card label="CPU(整機)" value={sys ? `${sys.cpu_pct.toFixed(0)} %` : "—"}
              sub={procs(["tingyi", "llama-server", "whisper-server"].map((n) => [n, proc(n)?.cpu_pct]), "%")} />
            <Card label="記憶體" value={sys ? `${(sys.mem_used_mb / 1000).toFixed(1)} / ${(sys.mem_total_mb / 1000).toFixed(0)} GB` : "—"}
              sub={procs(["tingyi", "llama-server", "whisper-server"].map((n) => [n, proc(n)?.rss_mb]), "MB")} />
            <Card label="GPU 使用率" value={sys?.gpu ? `${sys.gpu.util_pct} %` : "無 NVIDIA 資料"} sub={sys?.gpu ? `${sys.gpu.temp_c} °C` : ""} />
            <Card label="顯示記憶體" value={sys?.gpu ? `${sys.gpu.mem_used_mib} / ${sys.gpu.mem_total_mib} MiB` : "—"}
              sub={sys?.gpu ? `${((sys.gpu.mem_used_mib / sys.gpu.mem_total_mib) * 100).toFixed(0)} %` : ""} />
          </div>
          <div className="grid grid-cols-7 gap-2">
            <Card label="音訊積壓" value={p ? `${p.audio_backlog} chunk` : "—"} sub={p ? `峰值 ${p.audio_backlog_max}${p.speaking ? " · 講話中" : ""}` : ""} warn={!!p && p.audio_backlog > 20} />
            <Card label="翻譯佇列" value={p ? `${p.mt_queue}` : "—"} sub={p ? `完成 ${p.mt_done} · 合併 ${p.merges}` : ""} warn={!!p && p.mt_queue > 2} />
            <Card label="辨識均值" value={p ? `${p.asr_avg_ms} ms` : "—"} sub={p ? `最近 ${p.asr_last_ms} ms · ${p.segments} 段` : ""} />
            <Card label="預覽均值" value={p ? `${p.preview_avg_ms} ms` : "—"} sub={p ? `${p.preview_count} 次` : ""} />
            <Card label="說話者均值" value={p ? `${p.speaker_avg_ms} ms` : "—"} sub={p ? `最近 ${p.speaker_last_ms} ms` : ""} />
            <Card label="譯文首字均值" value={p ? `${p.mt_first_avg_ms} ms` : "—"} sub={p ? `完成均值 ${p.mt_total_avg_ms} ms` : ""} />
            <Card label="回修" value={p ? `${p.refine_count} 窗` : "—"} sub={p ? `修 ${p.revised_sources} · 補 ${p.recovered} · 重翻 ${p.refine_mt_done} · ${p.refine_avg_ms} ms` : ""} />
          </div>
          <div className="flex items-center gap-3">
            <div className="text-neutral-500 shrink-0">端到端首字(最近 {p?.e2e_recent.length ?? 0} 句,p50 {p?.e2e_p50 ?? "—"} ms)</div>
            <Sparkline values={p?.e2e_recent ?? []} />
            <button className="ml-auto rounded border border-neutral-700 bg-neutral-900 px-2 py-1 hover:bg-neutral-800" onClick={exportDiag}>
              匯出診斷
            </button>
          </div>
          {exportMsg && <div className="text-neutral-500 truncate">{exportMsg}</div>}
          {!running && <div className="text-neutral-600">執行中按「匯出診斷」才會包含管線統計。</div>}
        </div>
      )}
    </div>
  );
}

function procs(items: [string, number | undefined][], unit: string) {
  const parts = items.filter(([, v]) => v !== undefined).map(([n, v]) => `${n.replace("-server", "")} ${Math.round(v as number)}${unit === "%" ? "%" : ` ${unit}`}`);
  return parts.join(" · ");
}

function Card({ label, value, sub, warn }: { label: string; value: string; sub?: string; warn?: boolean }) {
  return (
    <div className="bg-neutral-900 rounded px-2 py-1 min-w-0">
      <div className="text-neutral-500 truncate">{label}</div>
      <div className={`text-sm font-mono ${warn ? "text-amber-400" : "text-neutral-100"}`}>{value}</div>
      {sub && <div className="text-neutral-500 truncate" title={sub}>{sub}</div>}
    </div>
  );
}

/** 純 SVG 折線:x 為句序、y 為毫秒;2000 ms 目標線。 */
function Sparkline({ values }: { values: number[] }) {
  const w = 360;
  const h = 40;
  const max = Math.max(2200, ...values);
  const pts = values.map((v, i) => {
    const x = values.length <= 1 ? 0 : (i / (values.length - 1)) * (w - 2) + 1;
    const y = h - 1 - (Math.min(v, max) / max) * (h - 2);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const yTarget = h - 1 - (2000 / max) * (h - 2);
  return (
    <svg width={w} height={h} className="shrink-0">
      <rect x={0} y={0} width={w} height={h} fill="#171717" rx={3} />
      <line x1={0} x2={w} y1={yTarget} y2={yTarget} stroke="#b45309" strokeDasharray="3 3" strokeWidth={1} />
      {pts.length > 1 && <polyline points={pts.join(" ")} fill="none" stroke="#38bdf8" strokeWidth={1.5} />}
      {pts.length === 1 && <circle cx={1} cy={Number(pts[0].split(",")[1])} r={2} fill="#38bdf8" />}
    </svg>
  );
}
