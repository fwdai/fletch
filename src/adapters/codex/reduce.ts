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

import type { ChatItem, RawEvent } from "../types";
import { asRecord } from "../shared/json";
import {
  dedupAgainstLast,
  finalizeStreamingItems,
  upsertToolCall,
} from "../shared/reducer-helpers";

/** Human label for a tool-call item. */
function toolName(item: Record<string, unknown>): string {
  const type = typeof item.type === "string" ? item.type : "";
  if (type === "command_execution") return "shell";
  if (type === "mcp_tool_call") {
    const server = typeof item.server === "string" ? item.server : "";
    const tool = typeof item.tool === "string" ? item.tool : "tool";
    return server ? `${server}.${tool}` : tool;
  }
  return type || "tool";
}

/** The input/arguments to display for a tool-call item. */
function toolInput(item: Record<string, unknown>): unknown {
  if (item.type === "command_execution") return item.command ?? "";
  if (item.type === "mcp_tool_call") return item.arguments ?? {};
  return {};
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
  return "";
}

const TOOL_TYPES = new Set(["command_execution", "mcp_tool_call"]);

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
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
      const items = finalizeStreamingItems(prev);
      return [...items, { kind: "notice", subtype: "turn_end", text: "success" }];
    }

    // `turn.failed` / `error` surface as a visible error notice.
    case "turn.failed":
    case "error": {
      const items = finalizeStreamingItems(prev);
      const message =
        typeof ev.message === "string"
          ? ev.message
          : typeof (asRecord(ev.error) as { message?: unknown }).message ===
              "string"
            ? String((asRecord(ev.error) as { message: string }).message)
            : "Codex reported an error.";
      return [
        ...items,
        { kind: "notice", subtype: "error", text: message, is_error: true },
      ];
    }

    default:
      return prev;
  }
}
