import { describe, expect, it } from 'vitest';
import { applyHostReply, HOST_NAME, type HostReply } from '../src/background/appLink.ts';
import { createSlotPicker, withSlot } from '../src/background/slots.ts';
import { DEFAULT_SETTINGS, type Settings } from '../src/shared/settings.ts';

const LLAMA = { baseUrl: 'http://127.0.0.1:51234/v1', model: 'hy-mt2', apiKey: 'key-abc' };

function reply(extra: Partial<Extract<HostReply, { ok: true }>> = {}): HostReply {
  return { ok: true, status: 'ready', llama: LLAMA, bridge: null, appVersion: '0.1.0', ...extra };
}

describe('網頁翻譯 slot 分工', () => {
  // T18
  it('有 slots 時輪流帶 id_slot,並存下回覆的 slots', () => {
    const pick = createSlotPicker([2, 3]);
    expect([pick(), pick(), pick()]).toEqual([2, 3, 2]);

    const body = { a: 1 };
    expect(withSlot(body, 2)).toEqual({ a: 1, id_slot: 2 });
    expect(body).toEqual({ a: 1 }); // 不改動傳入的物件

    const next = applyHostReply(DEFAULT_SETTINGS, reply({ slots: [2, 3] }));
    expect(next.direct.slots).toEqual([2, 3]);
  });

  // T19
  it('沒有 slots 時不帶 id_slot;舊版回覆存空陣列;手動模式不變', () => {
    for (const slots of [[], undefined]) {
      const pick = createSlotPicker(slots);
      expect([pick(), pick()]).toEqual([undefined, undefined]);
    }

    const out = withSlot({ a: 1 }, undefined);
    expect(out).toEqual({ a: 1 });
    expect('id_slot' in out).toBe(false);

    // 先放入舊的 slots,確認舊版回覆(沒有 slots 欄位)會把它清成 []
    const before: Settings = { ...DEFAULT_SETTINGS, direct: { ...DEFAULT_SETTINGS.direct, slots: [2, 3] } };
    expect(applyHostReply(before, reply()).direct.slots).toEqual([]);

    const manual: Settings = { ...before, connection: 'manual' };
    expect(applyHostReply(manual, reply({ slots: [5, 6] }))).toEqual(manual);
  });

  // T20
  it('擴充使用新的 native host 名稱', () => {
    expect(HOST_NAME).toBe('com.jerry.tingyi.host');
  });
});
