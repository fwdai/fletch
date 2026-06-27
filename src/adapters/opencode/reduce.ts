// Reducer for OpenCode's `opencode run --format json` event stream.
//
// Verified against opencode 1.15.12. OpenCode emits its OWN step/part model
// (not Claude- or Codex-shaped), one JSON object per line, with the session
// id repeated on every event's top-level `sessionID` (captured in the Rust
// transport, ignored here):
//
//   {"type":"step_start", "part":{"type":"step-start", …}}
//   {"type":"text",       "part":{"type":"text","text":"…","time":{start,end}}}
//   {"type":"tool_use",   "part":{"type":"tool","tool":"bash","callID":"…",
//                                 "state":{"status":"completed",
//                                          "input":{…},"output":"…",
//                                          "metadata":{"exit":0,…}}}}
//   {"type":"step_finish","part":{"reason":"tool-calls" | "stop", …}}
//
// In `run` (non-interactive) mode each `text` part is emitted once, fully
// formed (`time.end` is set) — there are no token-level deltas — so we append
// it whole. A turn is one or more steps; each tool step ends with
// `step_finish:tool-calls`, and the final step ends with `step_finish:stop`,
// which is the end-of-turn signal.

import { asRecord } from "../shared/json";
import {
  aliasToolInput,
  dedupAgainstLast,
  finalizeStreamingItems,
  upsertToolCall,
} from "../shared/reducer-helpers";
import type { ChatItem, RawEvent } from "../types";

/** OpenCode uses camelCase file fields (`filePath`/`oldString`/`newString`)
 *  while the shared tool presenters read Claude's snake_case names. Alias
 *  them so Read/Write/Edit render the path + diff without bespoke
 *  presenters. */
function normalizeToolInput(input: unknown): unknown {
  return aliasToolInput(input, [
    ["filePath", "file_path"],
    ["oldString", "old_string"],
    ["newString", "new_string"],
  ]);
}

/** The result text to show for a finished tool. */
function toolOutput(state: Record<string, unknown>): unknown {
  return typeof state.output === "string" ? state.output : "";
}

/** Did a finished tool fail? A shell command that ran but exited non-zero is
 *  flagged too (OpenCode keeps `status: "completed"` and records the code in
 *  `metadata.exit`). */
function isToolError(state: Record<string, unknown>): boolean {
  if (state.status === "error") return true;
  const meta = asRecord(state.metadata);
  return typeof meta.exit === "number" && meta.exit !== 0;
}

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const type = typeof ev.type === "string" ? ev.type : undefined;

  switch (type) {
    // Step boundaries carry no renderable content on their own.
    case "step_start":
      return prev;

    // The user's prompt. Never emitted live (Quorum injects the provider-
    // agnostic user_message there); normalizeTranscript synthesizes it from a
    // user message's text part during transcript replay.
    case "user_message": {
      const text = typeof ev.text === "string" ? ev.text : "";
      if (!text) return prev;
      return dedupAgainstLast(prev, { kind: "user_message", text });
    }

    // Finalized assistant text for a step (no streaming deltas in run mode).
    case "text": {
      const part = asRecord(ev.part);
      const text = typeof part.text === "string" ? part.text : "";
      if (!text) return prev;
      const items = finalizeStreamingItems(prev);
      // `model` is attached by normalizeTranscript from the parent message
      // blob's `modelID`; absent on the live run stream (which omits it).
      const model = typeof ev.model === "string" ? ev.model : undefined;
      return dedupAgainstLast(items, { kind: "agent_message", text, model });
    }

    // A reasoning part (`opencode run --thinking`, verified against 1.15.12):
    // {"type":"reasoning","part":{"type":"reasoning","text":"…"}} — same
    // whole-content shape as a `text` part. Surface it as a thinking notice.
    case "reasoning": {
      const part = asRecord(ev.part);
      const text = typeof part.text === "string" ? part.text : "";
      if (!text) return prev;
      return [...prev, { kind: "notice", subtype: "reasoning", text }];
    }

    // A tool call. `state.status` is `completed`/`error` once it's done, or
    // `pending`/`running` while in flight.
    case "tool_use": {
      const part = asRecord(ev.part);
      const id = typeof part.callID === "string" ? part.callID : "";
      if (!id) return prev;
      const name = typeof part.tool === "string" ? part.tool : "tool";
      const state = asRecord(part.state);
      const status = typeof state.status === "string" ? state.status : "";
      const input = normalizeToolInput(state.input);

      if (status === "completed" || status === "error") {
        let items = upsertToolCall(prev, {
          kind: "tool_call",
          id,
          name,
          input,
          streaming: false,
        });
        items = [
          ...items,
          {
            kind: "tool_result",
            tool_use_id: id,
            content: toolOutput(state),
            is_error: isToolError(state),
          },
        ];
        return items;
      }

      // pending / running — show it streaming until a later event settles it.
      return upsertToolCall(prev, {
        kind: "tool_call",
        id,
        name,
        input,
        streaming: true,
      });
    }

    case "step_finish": {
      const part = asRecord(ev.part);
      const reason = typeof part.reason === "string" ? part.reason : "";
      const items = finalizeStreamingItems(prev);
      // Only the turn's final step stops; intermediate `tool-calls` steps just
      // settle any still-streaming items.
      if (reason === "stop") {
        return [...items, { kind: "notice", subtype: "turn_end", text: "success" }];
      }
      return items;
    }

    // Defensive: surface a top-level error event as a visible notice.
    case "error": {
      const items = finalizeStreamingItems(prev);
      const message = typeof ev.message === "string" ? ev.message : "OpenCode reported an error.";
      return [...items, { kind: "notice", subtype: "error", text: message, is_error: true }];
    }

    default:
      return prev;
  }
}
