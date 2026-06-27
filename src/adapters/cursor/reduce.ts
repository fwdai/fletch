// Reducer for Cursor Agent's `cursor-agent -p --output-format stream-json`.
//
// Verified against cursor-agent 2026.05.24 (tool calls) and 2026.06.03
// (thinking). Cursor emits Claude Code's stream-json schema verbatim for
// system / user / assistant / result events — so those delegate to the
// Claude reducer. Two things differ, each its own dedicated event rather
// than a Claude content block:
//   1. Tool calls: a `tool_call` event with `started`/`completed` subtypes
//      and a typed payload, instead of `assistant.content[].tool_use` +
//      `user.content[].tool_result`, e.g.
//        {"type":"tool_call","subtype":"completed","call_id":"…",
//         "tool_call":{"shellToolCall":{"args":{"command":"…"},
//                      "result":{"success":{"exitCode":0,"stdout":"…",
//                                "interleavedOutput":"…"}}}}}
//   2. Thinking: a `thinking` event streamed as `subtype:"delta"` (each with
//      `text`) terminated by `subtype:"completed"`, instead of a thinking
//      content block on the `assistant` event.

import { reduce as claudeReduce } from "../claude/reduce";
import { asRecord } from "../shared/json";
import { aliasToolInput, upsertToolCall } from "../shared/reducer-helpers";
import type { ChatItem, RawEvent } from "../types";

/** Cursor names its file-tool fields differently from Claude — glob uses
 *  `globPattern`/`targetDirectory`, read/edit use `path`, edit carries the new
 *  content in `streamContent`. Alias them to the snake_case names the shared
 *  Glob/Read/Edit presenters read, so they render the pattern/path/diff
 *  instead of "(no pattern)"/"(no path)". (Grep already uses `pattern`/`path`,
 *  and shell goes through the command-string branch — both untouched.) */
function normalizeToolInput(input: unknown): unknown {
  return aliasToolInput(input, [
    ["globPattern", "pattern"],
    ["targetDirectory", "path"],
    ["path", "file_path"],
    ["streamContent", "new_string"],
  ]);
}

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
  // Shell calls read best as the command string; other tools show their args
  // (with field names aliased to what the shared presenters expect).
  const input =
    typeof args.command === "string" ? args.command : normalizeToolInput(inner.args ?? {});
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
      "failure" in result || (typeof success.exitCode === "number" && success.exitCode !== 0);
    const content =
      typeof success.interleavedOutput === "string"
        ? success.interleavedOutput
        : typeof success.stdout === "string"
          ? success.stdout
          : (inner.result ?? "");
    items = [...items, { kind: "tool_result", tool_use_id: id, content, is_error: isError }];
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

/** Cursor streams thinking as its own `thinking` event (subtype `delta` then
 *  `completed`), NOT a Claude content block — so it never reaches the Claude
 *  reducer. Accumulate the deltas into a single reasoning notice, appending to
 *  the trailing one while a block streams and starting a new one otherwise.
 *  `completed` carries no text and needs no handling. */
function handleThinking(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  if (ev.subtype !== "delta") return prev;
  const text = typeof ev.text === "string" ? ev.text : "";
  if (!text) return prev;
  const last = prev[prev.length - 1];
  if (last && last.kind === "notice" && last.subtype === "reasoning") {
    const next = prev.slice();
    next[next.length - 1] = { ...last, text: last.text + text };
    return next;
  }
  return [...prev, { kind: "notice", subtype: "reasoning", text }];
}

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  // Cursor-specific events; everything else is Claude-shaped.
  if (ev.type === "tool_call") return handleToolCall(prev, ev);
  if (ev.type === "thinking") return handleThinking(prev, ev);
  return claudeReduce(prev, ev);
}
