/**
 * 聽譯的共用 llama-server 依 slot 分工(字幕 0–1、網頁 2–3)。
 * 直連時輪流帶 `id_slot`,不佔字幕的 slot,也不洗掉它的提示快取。
 */

/** 依序輪用 slots 的選擇器;slots 為空時回 undefined(其他端點不認 id_slot) */
export function createSlotPicker(slots: readonly number[] | undefined): () => number | undefined {
  const list = slots ?? [];
  let i = 0;
  return () => (list.length === 0 ? undefined : list[i++ % list.length]);
}

/** 把 slot 加進 request body 的額外欄位;slot 為 undefined 時原樣回傳 */
export function withSlot(extraBody: Record<string, unknown>, slot: number | undefined): Record<string, unknown> {
  return slot === undefined ? extraBody : { ...extraBody, id_slot: slot };
}
