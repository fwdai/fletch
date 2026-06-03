// Reducer for Cursor Agent's `cursor-agent -p --output-format stream-json`.
//
// Verified against cursor-agent 2026.05.24. Cursor emits Claude Code's
// stream-json schema verbatim for system / user / assistant / result
// events — so those delegate to the Claude reducer. The one difference is
// tool calls: instead of Claude's `assistant.content[].tool_use` +
// `user.content[].tool_result`, Cursor emits a dedicated `tool_call` event
// with `started`/`completed` subtypes and a typed payload, e.g.
//   {"type":"tool_call","subtype":"completed","call_id":"…",
//    "tool_call":{"shellToolCall":{"args":{"command":"…"},
//                 "result":{"success":{"exitCode":0,"stdout":"…",
//                           "interleavedOutput":"…"}}}}}

import type { ChatItem, RawEvent } from "../types";
import { asRecord } from "../shared/json";
import { upsertToolCall } from "../shared/reducer-helpers";
import { reduce as claudeReduce } from "../claude/reduce";

/** Pull a renderable (name, input) out of Cursor's typed tool_call payload.
 *  The payload is a single-key object like `{shellToolCall: {args, result}}`. */
function toolCallParts(ev: RawEvent): {
  name: string;
  input: unknown;
  inner: Record<string, unknown>;
} {
  const tc = asRecord(ev.tool_call);
  const key = Object.keys(tc)[0] ?? "";
  const inner = asRecord(tc[key]);
  const name = key.replace(/ToolCall$/, "") || "tool";
  const args = asRecord(inner.args);
  // Shell calls read best as the command string; other tools show their args.
  const input = typeof args.command === "string" ? args.command : (inner.args ?? {});
  return { name, input, inner };
}

function handleToolCall(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const id = String(ev.call_id ?? "");
  if (!id) return prev;
  const subtype = typeof ev.subtype === "string" ? ev.subtype : "";
  const { name, input, inner } = toolCallParts(ev);

  if (subtype === "completed") {
    let items = upsertToolCall(prev, {
      kind: "tool_call",
      id,
      name,
      input,
      streaming: false,
    });
    const result = asRecord(inner.result);
    const success = asRecord(result.success);
    const isError =
      "failure" in result ||
      (typeof success.exitCode === "number" && success.exitCode !== 0);
    const content =
      typeof success.interleavedOutput === "string"
        ? success.interleavedOutput
        : typeof success.stdout === "string"
          ? success.stdout
          : (inner.result ?? "");
    items = [
      ...items,
      { kind: "tool_result", tool_use_id: id, content, is_error: isError },
    ];
    return items;
  }

  // "started" (and any other in-progress subtype): show it streaming.
  return upsertToolCall(prev, {
    kind: "tool_call",
    id,
    name,
    input,
    streaming: true,
  });
}

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  // Cursor-specific tool-call events; everything else is Claude-shaped.
  if (ev.type === "tool_call") return handleToolCall(prev, ev);
  return claudeReduce(prev, ev);
}
