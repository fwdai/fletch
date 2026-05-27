// Pure reducer over Claude Code's stream-json events. Given the current
// list of ChatItems and one raw event, returns the next list.
//
// Behavior matches the prior inline handler in src/store.ts, with the
// addition of the sanitizer pass on user-message text and the resulting
// slash_command / hook_output notices.

import type { ChatItem, RawEvent } from "../types";
import { asBlockList, asRecord } from "../shared/json";
import {
  appendToolInputDelta,
  dedupAgainstLast,
  extendLastAssistant,
  finalizeStreamingItems,
  upsertToolCall,
} from "../shared/reducer-helpers";
import { contentText } from "./content";
import { sanitizeUserText } from "./sanitize";

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const type = typeof ev.type === "string" ? ev.type : undefined;

  if (type === "stream_event") return handleStreamEvent(prev, ev);
  if (type === "assistant") return handleAssistant(prev, ev);
  if (type === "user") return handleUser(prev, ev);
  if (type === "result") return handleResult(prev, ev);
  return prev;
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
  let items = finalizeStreamingItems(prev);
  // The finalized assistant event arrives after the stream events for the
  // same turn. Anything already in items past the last user_message belongs
  // to this turn — check that range when deduping text blocks.
  const turnStart = lastIndexBy(items, (it) => it.kind === "user_message") + 1;

  for (const block of content) {
    if (block.type === "text" && typeof block.text === "string") {
      const exists = items
        .slice(turnStart)
        .some((it) => it.kind === "agent_message" && it.text === block.text);
      if (exists) continue;
      items = [...items, { kind: "agent_message", text: block.text }];
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
    const text = hasAssistantText
      ? `Turn failed (${subtype || "error"})`
      : resultText || `Turn failed (${subtype || "error"})`;
    items = [
      ...items,
      { kind: "notice", subtype: "error", text, is_error: true },
    ];
  } else if (!hasAssistantText && resultText.trim()) {
    items = [...items, { kind: "agent_message", text: resultText }];
  }

  items = [
    ...items,
    {
      kind: "notice",
      subtype: "turn_end",
      text: subtype || (isError ? "error" : "success"),
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
