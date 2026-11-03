import { describe, expect, it } from 'vitest';
import { initialLive, liveView, reduceLive, type LiveState } from '../src/content/liveSubs.ts';

type Ev = { type: string; [k: string]: unknown };

const run = (events: Ev[], from: LiveState = initialLive()) => events.reduce(reduceLive, from);
const tr = (id: number, text: string, will_translate = true): Ev => ({ type: 'transcript', id, text, will_translate });
const delta = (id: number, d: string): Ev => ({ type: 'translation_delta', id, delta: d });
const done = (id: number, text: string): Ev => ({ type: 'translation_done', id, text });

/** 深凍結:reducer 若直接改傳入的 state 會在嚴格模式拋錯 */
function deepFreeze<T>(o: T): T {
  if (o && typeof o === 'object' && !Object.isFrozen(o)) {
    Object.freeze(o);
    for (const v of Object.values(o as object)) deepFreeze(v);
  }
  return o;
}

describe('字幕事件整理成播放器內容', () => {
  // U16
  it('什麼都沒有時不顯示', () => {
    expect(liveView(initialLive())).toBeNull(); // 空白時也畫框會擋住影片
  });

  it('只有一句且沒譯文時顯示原文並標為等待中', () => {
    expect(liveView(run([tr(1, 'Hello')]))).toEqual({ text: 'Hello', partial: '', pending: true });
  });

  it('譯文 delta 逐段累加', () => {
    const v = liveView(run([tr(1, 'Hello'), delta(1, '你'), delta(1, '好')]));
    expect(v).toEqual({ text: '你好', partial: '', pending: false }); // 覆蓋而非累加會只剩最後一段
  });

  it('done 以完整譯文取代累加的片段', () => {
    const v = liveView(run([tr(1, 'Hello'), delta(1, '你'), done(1, '你好嗎')]));
    expect(v?.text).toBe('你好嗎');
    expect(v?.pending).toBe(false);
  });

  it('最新句還在等譯文時保留上一句', () => {
    const s = run([tr(1, 'One'), done(1, '一'), tr(2, 'Two')]);
    expect(liveView(s)?.text).toBe('一'); // 直接顯示新句原文會讓字幕在中英間閃爍
    expect(liveView(run([delta(2, '二')], s))?.text).toBe('二');
  });

  it('不翻譯的句子顯示原文', () => {
    expect(liveView(run([tr(1, 'Hello', false)]))).toEqual({ text: 'Hello', partial: '', pending: false });
    const s = run([tr(1, 'One'), done(1, '一'), tr(2, 'Two', false)]);
    expect(liveView(s)?.text).toBe('Two'); // 不翻的句子沒有譯文可等,不該一直卡在上一句
  });

  it('翻譯失敗顯示原文', () => {
    const v = liveView(run([tr(1, 'Hello'), { type: 'translation_failed', id: 1, error: 'boom' }]));
    expect(v).toEqual({ text: 'Hello', partial: '', pending: false });
  });

  it('回修譯文取代原譯文', () => {
    const v = liveView(run([tr(1, 'Hello'), done(1, '哈囉'), { type: 'translation_revised', id: 1, text: '你好' }]));
    expect(v?.text).toBe('你好');
  });

  it('句子合併後移除被併的句子並回到等譯文', () => {
    const s = run([tr(1, 'A'), done(1, '甲'), tr(2, 'B'), done(2, '乙'), { type: 'transcript_merged', into: 1, from: 2, text: 'A B' }]);
    expect(liveView(s)).toEqual({ text: 'A B', partial: '', pending: true }); // 沒移除 from 會顯示已被併掉的「乙」
    expect(liveView(run([done(1, '甲乙')], s))?.text).toBe('甲乙');
  });

  it('partial 是預覽原文,空字串清除', () => {
    const s = run([{ type: 'partial', text: 'abc' }]);
    expect(liveView(s)?.partial).toBe('abc');
    expect(liveView(run([tr(1, 'Hello'), done(1, '哈囉'), { type: 'partial', text: 'next' }]))).toEqual({ text: '哈囉', partial: 'next', pending: false });
    expect(liveView(run([{ type: 'partial', text: '' }], s))).toBeNull();
  });

  it('idle 後不顯示,新句子或新 partial 出現才恢復', () => {
    const idle = run([tr(1, 'Hello'), done(1, '哈囉'), { type: 'idle' }]);
    expect(liveView(idle)).toBeNull(); // 停頓後字幕不消失會一直掛在畫面上
    expect(liveView(run([tr(2, 'Next')], idle))).not.toBeNull();
    expect(liveView(run([{ type: 'partial', text: 'x' }], idle))?.partial).toBe('x');
    expect(liveView(run([{ type: 'partial', text: '' }], idle))).toBeNull();
  });

  it('未知事件被忽略', () => {
    const s = run([tr(1, 'Hello'), done(1, '哈囉')]);
    expect(liveView(run([{ type: 'level', rms: 0.3 }], s))).toEqual(liveView(s));
  });

  it('reducer 不改動傳入的 state', () => {
    const s = deepFreeze(run([tr(1, 'Hello'), delta(1, '你')]));
    const before = liveView(s);
    const events: Ev[] = [
      tr(2, 'x'),
      delta(1, '好'),
      done(1, '你好'),
      { type: 'translation_revised', id: 1, text: 'r' },
      { type: 'transcript_merged', into: 1, from: 2, text: 'm' },
      { type: 'partial', text: 'p' },
      { type: 'idle' },
    ];
    for (const e of events) expect(() => reduceLive(s, e)).not.toThrow();
    expect(liveView(s)).toEqual(before);
  });
});
