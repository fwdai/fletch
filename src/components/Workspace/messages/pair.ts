// Pure helper: groups each tool_call with its matching tool_result so
// the renderer can present them as one collapsible row. Orphan
// tool_results (no matching call, defensive case) pass through.

import type { ChatItem } from "@/store";
import type { ToolCall, ToolResult } from "./presenters/types";

export type ToolPair = { kind: "tool_pair"; call: ToolCall; result: ToolResult | null };
export type ViewItem = ChatItem | ToolPair;

/** Caller-owned identity cache keyed by tool_call id. Reused across renders so a
 *  tool_pair wrapper keeps its reference when its call+result are unchanged —
 *  lets React.memo(MessageItem) skip settled tool rows during a streaming delta.
 *  The reducer already preserves call/result identity; only the wrapper is new. */
export type PairCache = Map<string, ToolPair>;

export function pairToolItems(items: ChatItem[], cache?: PairCache): ViewItem[] {
  const consumed = new Set<number>();
  const out: ViewItem[] = [];
  const seen = cache ? new Set<string>() : null;

  for (let i = 0; i < items.length; i += 1) {
    if (consumed.has(i)) continue;
    const item = items[i];

    if (item.kind === "tool_call") {
      let result: ToolResult | null = null;
      for (let j = i + 1; j < items.length; j += 1) {
        const candidate = items[j];
        if (candidate.kind === "tool_result" && candidate.tool_use_id === item.id) {
          result = candidate;
          consumed.add(j);
          break;
        }
      }
      out.push(pairFor(item, result, cache, seen));
      continue;
    }

    out.push(item);
  }

  // Drop wrappers for tool_calls no longer present, keeping the cache bounded.
  if (cache && seen) for (const id of cache.keys()) if (!seen.has(id)) cache.delete(id);

  return out;
}

/** Reuse the cached wrapper when call+result references match; otherwise build a
 *  fresh one and cache it. Uncached (no cache passed) always builds fresh. */
function pairFor(
  call: ToolCall,
  result: ToolResult | null,
  cache: PairCache | undefined,
  seen: Set<string> | null,
): ToolPair {
  // Skip the cache for id-less calls (defensive bypass) — a shared "" key would
  // alias distinct rows onto one wrapper.
  if (!cache || !seen || !call.id) return { kind: "tool_pair", call, result };
  seen.add(call.id);
  const cached = cache.get(call.id);
  if (cached && cached.call === call && cached.result === result) return cached;
  const pair: ToolPair = { kind: "tool_pair", call, result };
  cache.set(call.id, pair);
  return pair;
}

/** Stable React key for a rendered row. Tool pairs/results key off their tool
 *  id so a row's expand state stays anchored to the tool rather than to a list
 *  position. Messages and notices have no id and the log is append-only, so
 *  their array index is a stable key — and keying them by text would remount on
 *  every streaming token. */
export function rowKey(item: ViewItem, index: number): string {
  switch (item.kind) {
    case "tool_pair":
      return item.call.id ? `tp:${item.call.id}` : `i:${index}`;
    // No `tool_call` case: pairToolItems wraps every tool_call into a
    // tool_pair, so only orphan tool_results reach here standalone.
    case "tool_result":
      return item.tool_use_id ? `tr:${item.tool_use_id}` : `i:${index}`;
    default:
      return `i:${index}`;
  }
}
