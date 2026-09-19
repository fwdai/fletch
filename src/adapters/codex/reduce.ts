// Reducer for Codex's `codex exec --json` event stream.
//
// Verified against codex-cli 0.135.0. Codex emits a thread / turn / item
// model (NOT the OpenAI Responses-API shapes an earlier stub guessed):
//
//   {"type":"thread.started","thread_id":"…"}            // session id
//   {"type":"turn.started"}
//   {"type":"item.started",  "item":{"id","type","status":"in_progress",…}}
//   {"type":"item.completed","item":{"id","type","status",…}}
//   {"type":"turn.completed","usage":{…}}                // end of turn
//
// Observed `item.type` values: `agent_message` ({text}), `command_execution`
// ({command, aggregated_output, exit_code, status}), `mcp_tool_call`
// ({server, tool, arguments, result, error, status}). There are no
// token-level text deltas in exec mode — items arrive whole, so assistant
// text and tool calls render on their `item.completed`.
//
// Multi-agent (`collaboration` tools, 0.153.4): the live stream carries ONLY
// `collab_tool_call` ({tool: "wait" | …, sender_thread_id, receiver_thread_ids,
// prompt, agents_states, status}) — the parent blocking on its sub-agents.
// The `spawn_agent` call and every sub-agent turn are absent from stdout; they
// exist only on disk, where the sync tags the child rollout's records with
// the spawn call's id (`parent_tool_use_id`) so replay nests them under it.

import { asRecord } from "@/adapters/shared/json";
import {
  dedupAgainstLast,
  endTurn,
  finalizeStreamingItems,
  upsertToolCall,
} from "@/adapters/shared/reducer-helpers";
import { withSubagentRouting } from "@/adapters/shared/subagents";
import type { ChatItem, RawEvent } from "@/adapters/types";

/** Human label for a tool-call item. */
function toolName(item: Record<string, unknown>): string {
  const type = typeof item.type === "string" ? item.type : "";
  if (type === "command_execution") return "shell";
  if (type === "mcp_tool_call") {
    const server = typeof item.server === "string" ? item.server : "";
    const tool = typeof item.tool === "string" ? item.tool : "tool";
    return server ? `${server}.${tool}` : tool;
  }
  // `collab.wait` / `collab.send_input` / …: the presenter matches the prefix.
  if (type === "collab_tool_call") return `collab.${item.tool ?? "tool"}`;
  return type || "tool";
}

/** The input/arguments to display for a tool-call item. */
function toolInput(item: Record<string, unknown>): unknown {
  if (item.type === "command_execution") return item.command ?? "";
  if (item.type === "mcp_tool_call") return item.arguments ?? {};
  if (item.type === "collab_tool_call") return collabInput(item);
  return {};
}

/** What a collab call was asked to do: the replayed function_call's arguments
 *  when there are any, else the live item's target/prompt fields — only those
 *  that carry something, since the live `wait` sets them all empty. */
function collabInput(item: Record<string, unknown>): unknown {
  if (item.arguments != null) return item.arguments;
  const out: Record<string, unknown> = {};
  for (const key of ["prompt", "receiver_thread_ids", "receiver_agents"]) {
    const v = item[key];
    if (v == null || (Array.isArray(v) && v.length === 0)) continue;
    out[key] = v;
  }
  return out;
}

/** Did a finished tool item fail? */
function isToolError(item: Record<string, unknown>): boolean {
  if (item.status === "failed") return true;
  if (item.type === "command_execution") {
    return typeof item.exit_code === "number" && item.exit_code !== 0;
  }
  if (item.type === "mcp_tool_call") {
    return item.error != null;
  }
  return false;
}

/** The result payload to show for a finished tool item. */
function toolResult(item: Record<string, unknown>): unknown {
  if (item.type === "command_execution") return item.aggregated_output ?? "";
  if (item.type === "mcp_tool_call") return item.error ?? item.result ?? "";
  if (item.type === "collab_tool_call") return item.result ?? "";
  return "";
}

const TOOL_TYPES = new Set(["command_execution", "mcp_tool_call", "collab_tool_call"]);

/** The human-readable message of an `error` / `turn.failed` event. Codex
 *  relays API failures as the raw response body serialized into `message`
 *  (`{"type":"error","status":400,"error":{"message":"…"}}`); unwrap that to
 *  the inner message so the notice reads as prose, not JSON. */
function errorMessage(ev: RawEvent): string {
  const raw =
    typeof ev.message === "string"
      ? ev.message
      : typeof asRecord(ev.error).message === "string"
        ? String(asRecord(ev.error).message)
        : "";
  if (!raw) return "Codex reported an error.";
  const trimmed = raw.trim();
  if (trimmed.startsWith("{")) {
    try {
      const body = asRecord(JSON.parse(trimmed));
      const inner = asRecord(body.error).message;
      if (typeof inner === "string" && inner) return inner;
      if (typeof body.message === "string" && body.message) return body.message;
    } catch {
      // Not JSON after all — show it as-is.
    }
  }
  return raw;
}

/** Append an error notice unless the log already ends with this exact one:
 *  `error` and the following `turn.failed` carry the same message. */
function appendErrorNotice(items: ChatItem[], text: string): ChatItem[] {
  const last = items[items.length - 1];
  if (last?.kind === "notice" && last.subtype === "error" && last.text === text) return items;
  return [...items, { kind: "notice", subtype: "error", text, is_error: true }];
}

// A replayed sub-agent record carries the spawn call's id; it reduces into
// that tool_call's children rather than the main timeline.
export const reduce = withSubagentRouting(reduceTop);

function reduceTop(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const type = typeof ev.type === "string" ? ev.type : undefined;

  switch (type) {
    // Session id capture happens in the Rust transport; nothing to render.
    // `thread.started` also re-fires on every per-turn process, so it must
    // never reset the transcript.
    case "thread.started":
    case "turn.started":
      return prev;

    // User turns never appear in the live `exec` stream (the composer adds
    // them optimistically on send) — they only arrive via transcript replay
    // (`normalizeTranscript` emits this synthetic shape from the rollout).
    case "user": {
      const text = typeof ev.text === "string" ? ev.text : "";
      if (!text) return prev;
      return dedupAgainstLast(prev, { kind: "user_message", text });
    }

    // A tool item begins executing — show it streaming until completion.
    case "item.started":
    case "item.updated": {
      const item = asRecord(ev.item);
      const id = typeof item.id === "string" ? item.id : "";
      const itemType = typeof item.type === "string" ? item.type : "";
      if (!id || !TOOL_TYPES.has(itemType)) return prev;
      return upsertToolCall(prev, {
        kind: "tool_call",
        id,
        name: toolName(item),
        input: toolInput(item),
        streaming: true,
      });
    }

    case "item.completed": {
      const item = asRecord(ev.item);
      const id = typeof item.id === "string" ? item.id : "";
      const itemType = typeof item.type === "string" ? item.type : "";

      if (itemType === "agent_message") {
        const items = finalizeStreamingItems(prev);
        const text = typeof item.text === "string" ? item.text : "";
        if (!text) return items;
        // `model` is attached by normalizeTranscript from the turn_context
        // record; absent on the live exec stream (which omits it).
        const model = typeof item.model === "string" ? item.model : undefined;
        return dedupAgainstLast(items, { kind: "agent_message", text, model });
      }

      if (itemType === "reasoning") {
        const text = typeof item.text === "string" ? item.text : "";
        if (!text) return prev;
        return [...prev, { kind: "notice", subtype: "reasoning", text }];
      }

      if (TOOL_TYPES.has(itemType) && id) {
        // Settle the (possibly streaming) tool_call with final args, then
        // append its result.
        let items = upsertToolCall(prev, {
          kind: "tool_call",
          id,
          name: toolName(item),
          input: toolInput(item),
          streaming: false,
        });
        items = [
          ...items,
          {
            kind: "tool_result",
            tool_use_id: id,
            content: toolResult(item),
            is_error: isToolError(item),
          },
        ];
        return items;
      }

      return prev;
    }

    case "turn.completed": {
      return endTurn(finalizeStreamingItems(prev));
    }

    // A failed model call (usage limit, auth, 4xx) arrives as `error` then
    // `turn.failed` with the same message, after which the process exits 1.
    // Show the message once and close the turn as an error — the same shape
    // claude's failed `result` produces — so the chat says why it stopped.
    case "error":
      return appendErrorNotice(finalizeStreamingItems(prev), errorMessage(ev));

    case "turn.failed": {
      const items = appendErrorNotice(finalizeStreamingItems(prev), errorMessage(ev));
      return [...items, { kind: "notice", subtype: "turn_end", text: "error" }];
    }

    default:
      return prev;
  }
}
