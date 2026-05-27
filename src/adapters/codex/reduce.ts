// TODO(codex-real-impl): event shapes here are inferred from OpenAI's
// public Responses-API streaming docs and are not yet verified against
// real `codex` CLI output. When codex's Rust transport ships and we can
// observe its actual events, replace these stubs with the verified
// mapping and add comprehensive fixtures.
//
// The interface contract is the same as Claude's reducer; only the
// per-event parsing differs.

import type { ChatItem, RawEvent } from "../types";
import { asRecord } from "../shared/json";
import {
  appendToolInputDelta,
  dedupAgainstLast,
  extendLastAssistant,
  finalizeStreamingItems,
  upsertToolCall,
} from "../shared/reducer-helpers";

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const type = typeof ev.type === "string" ? ev.type : undefined;

  // Responses-style streaming text deltas.
  if (type === "response.output_text.delta") {
    const delta = typeof ev.delta === "string" ? ev.delta : "";
    return delta ? extendLastAssistant(prev, delta) : prev;
  }

  // Finalized assistant text.
  if (type === "response.output_text.done" || type === "message") {
    let items = finalizeStreamingItems(prev);
    const text =
      typeof ev.text === "string"
        ? ev.text
        : typeof (ev as { content?: unknown }).content === "string"
          ? ((ev as { content: string }).content)
          : "";
    if (!text) return items;
    items = dedupAgainstLast(items, { kind: "agent_message", text });
    return items;
  }

  // Function/tool calls in Responses-API form.
  if (type === "response.function_call.started" || type === "function_call") {
    const id = String(ev.call_id ?? ev.id ?? "");
    if (!id) return prev;
    return upsertToolCall(prev, {
      kind: "tool_call",
      id,
      name: String(ev.name ?? "tool"),
      input: ev.arguments ?? "",
      streaming: type === "response.function_call.started",
    });
  }

  if (type === "response.function_call_arguments.delta") {
    const idx = typeof ev.output_index === "number" ? ev.output_index : 0;
    const partial = typeof ev.delta === "string" ? ev.delta : "";
    return partial ? appendToolInputDelta(prev, idx, partial) : prev;
  }

  if (type === "function_call_output") {
    const output = asRecord(ev.output);
    return [
      ...prev,
      {
        kind: "tool_result",
        tool_use_id: String(ev.call_id ?? ""),
        content: output.content ?? ev.output ?? "",
        is_error: output.is_error === true,
      },
    ];
  }

  // User echo (codex transcripts often include the user prompt).
  if (type === "user") {
    const text =
      typeof ev.content === "string"
        ? ev.content
        : typeof (asRecord(ev.message) as { content?: unknown }).content ===
            "string"
          ? String((asRecord(ev.message) as { content: string }).content)
          : "";
    if (!text) return prev;
    return dedupAgainstLast(prev, { kind: "user_message", text });
  }

  // Turn completion.
  if (type === "response.completed" || type === "result") {
    let items = finalizeStreamingItems(prev);
    items = [
      ...items,
      { kind: "notice", subtype: "turn_end", text: "success" },
    ];
    return items;
  }

  return prev;
}
