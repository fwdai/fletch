// Threading a session_record's `seq` from the record, through the events
// normalized from it, onto the chat items reduced from those events — so a
// rendered item knows its position in the record stream.
//
// Normalizers fan one record out into several events (and fold data from other
// records into them), so each stamps the seq of the record an event is emitted
// FOR via `fromRecord`. Reducers stay seq-unaware: `withRecordSeq` stamps the
// event's seq onto whatever items the event created, which covers every item
// kind and every helper (endTurn, upsertToolCall, sub-agent routing…) at once.

import type { ChatItem, RawEvent } from "@/adapters/types";

type Reducer = (prev: ChatItem[], ev: RawEvent) => ChatItem[];

/** `ev` as emitted for the record at `seq`. Copies rather than mutating: some
 *  normalizers pass the record body itself through as the event. A bare-body
 *  call (no seq) returns `ev` untouched. */
export function fromRecord(ev: RawEvent, seq: number | undefined): RawEvent {
  return seq === undefined ? ev : { ...ev, recordSeq: seq };
}

/** `reduce`, stamping a record event's `recordSeq` onto every item it created —
 *  including items nested into a sub-agent tool_call's `children`. Items it only
 *  updated keep the seq they already carry (reducers update by spreading), so a
 *  streaming message extended by later records stays at the record that opened
 *  it. Live events carry no seq and pass through untouched. Idempotent, so a
 *  reducer delegating to another wrapped one (cursor → claude) is safe. */
export function withRecordSeq(reduce: Reducer): Reducer {
  return (prev, ev) => {
    const next = reduce(prev, ev);
    const seq = ev.recordSeq;
    return typeof seq === "number" && next !== prev ? stampItems(prev, next, seq) : next;
  };
}

/** Stamp the items of `next` that `prev` doesn't hold by reference at the same
 *  index — the only ones a reduce can have created — leaving the rest (and the
 *  array itself, when nothing needed a seq) referentially intact. */
function stampItems(prev: ChatItem[], next: ChatItem[], seq: number): ChatItem[] {
  let out: ChatItem[] | null = null;
  for (let i = 0; i < next.length; i += 1) {
    const item = next[i];
    const before = prev[i];
    if (item === before) continue;
    const stamped = stampItem(item, before, seq);
    if (stamped !== item) {
      out ??= next.slice();
      out[i] = stamped;
    }
  }
  return out ?? next;
}

function stampItem(item: ChatItem, before: ChatItem | undefined, seq: number): ChatItem {
  let result = item;
  if (item.kind === "tool_call" && item.children) {
    const prevChildren =
      before?.kind === "tool_call" && before.id === item.id ? (before.children ?? []) : [];
    const children = stampItems(prevChildren, item.children, seq);
    if (children !== item.children) result = { ...item, children };
  }
  return result.recordSeq === undefined ? { ...result, recordSeq: seq } : result;
}
