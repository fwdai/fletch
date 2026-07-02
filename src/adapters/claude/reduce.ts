// Pure reducer over Claude Code's stream-json events. Given the current
// list of ChatItems and one raw event, returns the next list.
//
// Behavior matches the prior inline handler in src/store.ts, with the
// addition of the sanitizer pass on user-message text and the resulting
// slash_command / hook_output notices.

import { asBlockList, asRecord } from "@/adapters/shared/json";
import {
  appendToolInputDelta,
  dedupAgainstLast,
  extendLastAssistant,
  finalizeStreamingItems,
  upsertToolCall,
} from "@/adapters/shared/reducer-helpers";
import type { ChatItem, RawEvent } from "@/adapters/types";
import { contentText } from "./content";
import { sanitizeUserText } from "./sanitize";

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  // Subagent (sidechain) events are tagged with the parent Task/Agent
  // tool_use id; route them under that tool_call's nested log instead of the
  // main timeline. Main-agent events have no parent and reduce normally.
  const parentId = parentToolUseId(ev);
  if (parentId) return routeToChild(prev, parentId, ev);
  return reduceTop(prev, ev);
}

function reduceTop(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const type = typeof ev.type === "string" ? ev.type : undefined;

  if (type === "stream_event") return handleStreamEvent(prev, ev);
  if (type === "assistant") return handleAssistant(prev, ev);
  if (type === "user") return handleUser(prev, ev);
  if (type === "result") return handleResult(prev, ev);
  return prev;
}

/** Claude's stream-json tags every event belonging to a subagent with the
 *  spawning Task/Agent tool_use id (top-level `parent_tool_use_id`, set on
 *  user/assistant/result/stream_event envelopes alike). Null/absent for the
 *  main agent. */
function parentToolUseId(ev: RawEvent): string | null {
  const v = ev.parent_tool_use_id;
  return typeof v === "string" && v.length > 0 ? v : null;
}

/** Fold a sidechain event into the children of the tool_call it belongs to,
 *  reducing it there with the same top-level logic. Searches nested
 *  tool_calls so a subagent that itself spawns a subagent threads correctly.
 *  If the parent tool_call isn't present yet (ordering race), returns `items`
 *  unchanged — the event is dropped rather than leaked into the main log. */
function routeToChild(items: ChatItem[], parentId: string, ev: RawEvent): ChatItem[] {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind !== "tool_call") continue;
    if (it.id === parentId) {
      const next = items.slice();
      next[i] = { ...it, children: reduceTop(it.children ?? [], ev) };
      return next;
    }
    if (it.children && it.children.length > 0) {
      const updated = routeToChild(it.children, parentId, ev);
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

function handleStreamEvent(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const inner = asRecord(ev.event);
  const innerType = inner.type;

  if (innerType === "content_block_start") {
    const block = asRecord(inner.content_block);
    if (block.type === "text" && typeof block.text === "string" && block.text) {
      return extendLastAssistant(prev, block.text);
    }
    if (block.type === "tool_use") {
      return upsertToolCall(prev, {
        kind: "tool_call",
        id: String(block.id ?? ""),
        name: String(block.name ?? "tool"),
        input: block.input ?? "",
        streaming: true,
      });
    }
    return prev;
  }

  const delta = asRecord(inner.delta);
  if (delta.type === "text_delta" && typeof delta.text === "string") {
    return extendLastAssistant(prev, delta.text);
  }
  if (
    delta.type === "input_json_delta" &&
    typeof delta.partial_json === "string" &&
    typeof inner.index === "number"
  ) {
    return appendToolInputDelta(prev, inner.index, delta.partial_json);
  }
  return prev;
}

function handleAssistant(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const message = asRecord(ev.message);
  const content = asBlockList(message.content);
  // The model that produced this turn (Claude reports it on the finalized
  // assistant event). Stamped onto the turn's agent_message so the UI can show
  // the actual model in use; absent on stream deltas, so a turn that streamed
  // in gets its model only once this finalized event arrives.
  const model = typeof message.model === "string" ? message.model : undefined;
  let items = finalizeStreamingItems(prev);
  // The finalized assistant event arrives after the stream events for the
  // same turn. Anything already in items past the last user_message belongs
  // to this turn — check that range when deduping text blocks.
  const turnStart = lastIndexBy(items, (it) => it.kind === "user_message") + 1;

  for (const block of content) {
    if (block.type === "thinking" && typeof block.thinking === "string") {
      // Extended-thinking block → reasoning notice (we capture the whole
      // block here; the stream-event deltas are ignored). The text lives in
      // the assistant event's `thinking` field — confirmed against real
      // persisted events. (Some Claude auth modes redact this to "" and emit
      // only a signature; `if (!text) continue` skips those cleanly.)
      const text = block.thinking;
      if (!text) continue;
      const exists = items
        .slice(turnStart)
        .some((it) => it.kind === "notice" && it.subtype === "reasoning" && it.text === text);
      if (exists) continue;
      items = [...items, { kind: "notice", subtype: "reasoning", text }];
    } else if (block.type === "text" && typeof block.text === "string") {
      // The text may already exist as a streamed agent_message for this turn
      // (deltas arrived before this finalized event). If so, stamp the model
      // onto it rather than appending a duplicate; otherwise append fresh.
      const existingIdx = items
        .slice(turnStart)
        .findIndex((it) => it.kind === "agent_message" && it.text === block.text);
      if (existingIdx !== -1) {
        const absIdx = turnStart + existingIdx;
        const existing = items[absIdx];
        if (existing.kind === "agent_message" && model && existing.model !== model) {
          items = items.slice();
          items[absIdx] = { ...existing, model };
        }
        continue;
      }
      items = [...items, { kind: "agent_message", text: block.text, streaming: false, model }];
    } else if (block.type === "tool_use") {
      items = upsertToolCall(items, {
        kind: "tool_call",
        id: String(block.id ?? ""),
        name: String(block.name ?? "tool"),
        input: block.input,
      });
    }
  }
  return items;
}

function handleUser(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const message = asRecord(ev.message);
  const rawText = contentText(message.content);

  let items = prev;
  if (rawText) {
    const { text, notices } = sanitizeUserText(rawText);
    if (text) {
      items = dedupAgainstLast(items, { kind: "user_message", text });
    }
    for (const notice of notices) {
      items = [...items, notice];
    }
  }

  // tool_result blocks ride along inside user messages.
  if (Array.isArray(message.content)) {
    for (const block of asBlockList(message.content)) {
      if (block.type === "tool_result") {
        items = [
          ...items,
          {
            kind: "tool_result",
            tool_use_id: String(block.tool_use_id ?? ""),
            content: block.content,
            is_error: block.is_error === true,
          },
        ];
      }
    }
  }
  return items;
}

function handleResult(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const isError = ev.is_error === true;
  const subtype = String(ev.subtype ?? "");
  const resultText = typeof ev.result === "string" ? ev.result : "";

  let items = finalizeStreamingItems(prev);

  // Has the agent already emitted text since the last user turn? If not,
  // surface the result string as an agent_message so the turn isn't blank.
  const lastUserIdx = lastIndexBy(items, (it) => it.kind === "user_message");
  const hasAssistantText = items
    .slice(lastUserIdx + 1)
    .some((it) => it.kind === "agent_message" && it.text.trim().length > 0);

  if (isError) {
    // Surface the real error detail (e.g. an auth/401 message) rather than the
    // result envelope's `subtype` — claude reports subtype "success" even when
    // the turn errored, which produced the contradictory "Turn failed
    // (success)". When the agent already spoke (the detail is on screen), keep
    // the notice to a clean label instead of repeating it.
    const text = hasAssistantText ? "Turn failed" : resultText.trim() || "Turn failed";
    items = [...items, { kind: "notice", subtype: "error", text, is_error: true }];
  } else if (!hasAssistantText && resultText.trim()) {
    items = [...items, { kind: "agent_message", text: resultText }];
  }

  items = [
    ...items,
    {
      kind: "notice",
      subtype: "turn_end",
      text: isError ? "error" : subtype || "success",
    },
  ];
  return items;
}

function lastIndexBy<T>(items: T[], pred: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (pred(items[i])) return i;
  }
  return -1;
}
