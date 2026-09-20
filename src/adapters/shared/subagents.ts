// Nesting a sub-agent's events under the tool call that spawned it.
//
// Claude's stream-json tags every sidechain envelope with the spawning
// Task/Agent tool_use id (top-level `parent_tool_use_id`), and the transcript
// sync stamps the same field onto the persisted records of every provider that
// writes sub-agents to files of their own (Claude's `subagents/agent-*.jsonl`,
// Codex's child rollouts). Any reducer whose events can carry that tag routes
// through here, so the nesting logic exists once.

import type { ChatItem, RawEvent } from "@/adapters/types";

type Reducer = (prev: ChatItem[], ev: RawEvent) => ChatItem[];

/** The spawning tool_use id an event is tagged with; null for the main agent. */
export function parentToolUseId(ev: RawEvent): string | null {
  const v = ev.parent_tool_use_id;
  return typeof v === "string" && v.length > 0 ? v : null;
}

/** Fold a sub-agent event into the children of the tool_call it belongs to,
 *  reducing it there with the provider's own top-level logic. Searches nested
 *  tool_calls so a sub-agent that itself spawns a sub-agent threads correctly.
 *  If the parent tool_call isn't present yet (ordering race), returns `items`
 *  unchanged — the event is dropped rather than leaked into the main log. */
export function routeToChild(
  items: ChatItem[],
  parentId: string,
  ev: RawEvent,
  reduceTop: Reducer,
): ChatItem[] {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind !== "tool_call") continue;
    if (it.id === parentId) {
      const next = items.slice();
      next[i] = { ...it, children: reduceTop(it.children ?? [], ev) };
      return next;
    }
    if (it.children && it.children.length > 0) {
      const updated = routeToChild(it.children, parentId, ev, reduceTop);
      // routeToChild returns the same array reference when it finds no match,
      // so an identity change means the parent lived inside these children.
      if (updated !== it.children) {
        const next = items.slice();
        next[i] = { ...it, children: updated };
        return next;
      }
    }
  }
  return items;
}

/** A provider's reducer with sub-agent routing in front: tagged events go to
 *  their tool_call's children, everything else to `reduceTop` as before. */
export function withSubagentRouting(reduceTop: Reducer): Reducer {
  return (prev, ev) => {
    const parentId = parentToolUseId(ev);
    return parentId ? routeToChild(prev, parentId, ev, reduceTop) : reduceTop(prev, ev);
  };
}
