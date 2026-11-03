/**
 * 聽譯送來的字幕事件(PipelineEvent)整理成播放器上要顯示的一句。規則照聽譯懸浮字幕(src/Overlay.tsx):
 * 只顯示最新一句;新句還在等譯文時保留上一句,畫面不閃成空白。
 */

interface Line {
  id: number;
  source: string;
  translation: string;
  /** translating:等譯文;done:已翻好;plain:不翻或翻譯失敗,顯示原文 */
  state: 'translating' | 'done' | 'plain';
}

export interface LiveState {
  lines: Line[];
  partial: string;
  /** 聽譯判定閒置後收起,直到有新內容 */
  hidden: boolean;
}

/** 只留最近幾句就夠判斷要顯示哪一句 */
const KEEP = 4;

export function initialLive(): LiveState {
  return { lines: [], partial: '', hidden: false };
}

type Ev = { type: string; [k: string]: unknown };

const num = (v: unknown) => Number(v);
const str = (v: unknown) => (typeof v === 'string' ? v : '');

function update(s: LiveState, id: number, f: (l: Line) => Line): LiveState {
  return { ...s, lines: s.lines.map((l) => (l.id === id ? f(l) : l)) };
}

export function reduceLive(s: LiveState, ev: Ev): LiveState {
  switch (ev.type) {
    case 'transcript': {
      const line: Line = { id: num(ev.id), source: str(ev.text), translation: '', state: ev.will_translate === false ? 'plain' : 'translating' };
      return { lines: [...s.lines.filter((l) => l.id !== line.id), line].slice(-KEEP), partial: s.partial, hidden: false };
    }
    case 'translation_delta':
      return update(s, num(ev.id), (l) => ({ ...l, translation: l.translation + str(ev.delta) }));
    case 'translation_done':
      return update(s, num(ev.id), (l) => ({ ...l, translation: str(ev.text), state: 'done' }));
    case 'translation_revised':
      return update(s, num(ev.id), (l) => ({ ...l, translation: str(ev.text), state: 'done' }));
    case 'translation_failed':
      return update(s, num(ev.id), (l) => ({ ...l, translation: '', state: 'plain' }));
    case 'transcript_merged': {
      const into = num(ev.into);
      const from = num(ev.from);
      const lines = s.lines.filter((l) => l.id !== from).map((l): Line => (l.id === into ? { ...l, source: str(ev.text), translation: '', state: 'translating' } : l));
      return { ...s, lines };
    }
    case 'partial': {
      const text = str(ev.text);
      return { ...s, partial: text, hidden: s.hidden && text === '' };
    }
    case 'idle':
      return { ...s, partial: '', hidden: true };
    default:
      return s;
  }
}

function textOf(l: Line): string {
  return l.state === 'plain' ? l.source : l.translation || l.source;
}

const hasText = (l: Line) => l.state === 'plain' || l.translation !== '';

export function liveView(s: LiveState): { text: string; partial: string; pending: boolean } | null {
  if (s.hidden) return null;
  const newest = s.lines[s.lines.length - 1];
  const prev = s.lines[s.lines.length - 2];
  if (!newest) return s.partial ? { text: '', partial: s.partial, pending: false } : null;
  const shown = !hasText(newest) && prev ? prev : newest;
  return { text: textOf(shown), partial: s.partial, pending: shown.state === 'translating' && shown.translation === '' };
}
