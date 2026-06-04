import type { ChatItem } from "../types";
import { asRecord } from "./json";

// Walk from the end since the items we care about (latest streaming
// agent_message, latest tool_call) are always near the tail.
function findLastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (predicate(items[i])) return i;
  }
  return -1;
}

/** Append text to the most recent streaming agent_message, or start a new one.
 *  Walks back across non-agent items (e.g. interleaved tool_calls) — claude
 *  emits text/tool_use as separate content blocks within one turn, and a
 *  text delta arriving after a tool_use start still belongs to the agent
 *  message that's still streaming. */
export function extendLastAssistant(
  items: ChatItem[],
  appendText: string,
): ChatItem[] {
  const idx = findLastIndex(
    items,
    (it) => it.kind === "agent_message" && it.streaming === true,
  );
  if (idx !== -1) {
    const item = items[idx];
    if (item.kind === "agent_message") {
      const next = items.slice();
      next[idx] = { ...item, text: item.text + appendText };
      return next;
    }
  }
  return [
    ...items,
    { kind: "agent_message", text: appendText, streaming: true },
  ];
}

/** Clear the `streaming` flag on every streaming agent_message and tool_call.
 *  Called when claude emits the finalized `assistant` event — by that point
 *  all stream-deltas for the turn are done. */
export function finalizeStreamingItems(items: ChatItem[]): ChatItem[] {
  let mutated = false;
  const next = items.map((item) => {
    if (item.kind === "agent_message" && item.streaming) {
      mutated = true;
      const { streaming: _s, ...rest } = item;
      return { ...rest, streaming: false };
    }
    if (item.kind === "tool_call" && item.streaming) {
      mutated = true;
      const { streaming: _s, ...rest } = item;
      return { ...rest, streaming: false };
    }
    return item;
  });
  return mutated ? next : items;
}

/** Upsert a tool_call by id. Streaming flag is preserved from the caller. */
export function upsertToolCall(
  items: ChatItem[],
  tool: Extract<ChatItem, { kind: "tool_call" }>,
): ChatItem[] {
  const idx = items.findIndex(
    (item) => item.kind === "tool_call" && item.id === tool.id,
  );
  if (idx === -1) return [...items, tool];
  const next = items.slice();
  next[idx] = { ...tool };
  return next;
}

/**
 * Append partial JSON to the input of a streaming tool_call. `index` is the
 * positional index *among tool_call items* (Claude's `stream_event.index`),
 * not the absolute items array index. Falls back to the last tool_call if
 * the positional lookup misses (rare ordering races).
 */
export function appendToolInputDelta(
  items: ChatItem[],
  toolCallIndex: number,
  partialJson: string,
): ChatItem[] {
  let seen = -1;
  let idx = items.findIndex((item) => {
    if (item.kind !== "tool_call") return false;
    seen += 1;
    return seen === toolCallIndex;
  });
  if (idx === -1) {
    idx = findLastIndex(items, (item) => item.kind === "tool_call");
  }
  if (idx === -1) return items;
  const item = items[idx];
  if (item.kind !== "tool_call") return items;
  const input =
    typeof item.input === "string" ? item.input + partialJson : partialJson;
  const next = items.slice();
  next[idx] = { ...item, input };
  return next;
}

/** Skip the append if the tail item is identical kind+text. Used for dedup
 *  against live mode (the user message is appended pre-echo) and against
 *  transcript replay. Only works for kinds that carry a `text` field. */
export function dedupAgainstLast(
  items: ChatItem[],
  candidate: Extract<ChatItem, { kind: "user_message" | "agent_message" }>,
): ChatItem[] {
  const last = items[items.length - 1];
  if (last && last.kind === candidate.kind && last.text === candidate.text) {
    return items;
  }
  // Suppress an echoed user_message whose text matches the slash_command
  // notice we optimistically inserted when sending. The notice text is
  // `/<name>` with no args; allow `/<name>` or `/<name> <args>` echoes.
  if (
    candidate.kind === "user_message" &&
    last &&
    last.kind === "notice" &&
    last.subtype === "slash_command" &&
    (candidate.text === last.text ||
      candidate.text.startsWith(last.text + " "))
  ) {
    return items;
  }
  return [...items, candidate];
}

/** Alias tool-input fields so the shared Read/Write/Edit presenters — which
 *  read Claude's snake_case names (`file_path`, `old_string`, …) — render
 *  correctly for agents that use different field names. For each
 *  `[from, to]` pair, copies a string `from` value to `to` when `to` isn't
 *  already set. Returns the input untouched (same reference) when there's
 *  nothing to alias, so callers can pass through non-file tools (e.g. bash's
 *  `command`) for free. */
export function aliasToolInput(
  input: unknown,
  aliases: ReadonlyArray<readonly [from: string, to: string]>,
): unknown {
  const rec = asRecord(input);
  let out: Record<string, unknown> | null = null;
  for (const [from, to] of aliases) {
    if (typeof rec[from] === "string" && rec[to] === undefined) {
      out = out ?? { ...rec };
      out[to] = rec[from];
    }
  }
  return out ?? input;
}
