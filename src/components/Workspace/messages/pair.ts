// Pure helper: groups each tool_call with its matching tool_result so
// the renderer can present them as one collapsible row. Orphan
// tool_results (no matching call, defensive case) pass through.

import type { ChatItem } from "@/store";
import type { ToolCall, ToolResult } from "./presenters/types";

export type ViewItem = ChatItem | { kind: "tool_pair"; call: ToolCall; result: ToolResult | null };

export function pairToolItems(items: ChatItem[]): ViewItem[] {
  const consumed = new Set<number>();
  const out: ViewItem[] = [];

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
      out.push({ kind: "tool_pair", call: item, result });
      continue;
    }

    out.push(item);
  }

  return out;
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
