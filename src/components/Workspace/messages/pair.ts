// Pure helper: groups each tool_call with its matching tool_result so
// the renderer can present them as one collapsible row. Orphan
// tool_results (no matching call, defensive case) pass through.

import type { ChatItem } from "../../../store";
import type { ToolCall, ToolResult } from "./presenters/types";

export type ViewItem =
  | ChatItem
  | { kind: "tool_pair"; call: ToolCall; result: ToolResult | null };

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
        if (
          candidate.kind === "tool_result" &&
          candidate.tool_use_id === item.id
        ) {
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
