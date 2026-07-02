// Antigravity (agy) transcript replay.
//
// agy persists a structured transcript at
// `~/.gemini/antigravity-cli/brain/<conversationId>/.system_generated/logs/transcript_full.jsonl`.
// Each line is a "step" with a `type`:
//   - USER_INPUT          → the prompt, wrapped in <USER_REQUEST>…</USER_REQUEST>
//   - PLANNER_RESPONSE     → assistant `content` (markdown text) OR `tool_calls` (name+args)
//   - CONVERSATION_HISTORY → lifecycle marker (ignored)
//   - everything else (LIST_DIRECTORY/RUN_COMMAND/SEARCH_WEB/GENERIC/… — open-ended)
//                          → a tool RESULT, carried in `content`
//
// Tool calls and their results share no id on disk, so we pair them by order:
// each result claims the oldest unmatched call id (FIFO).

import { asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

const FIXED = new Set(["USER_INPUT", "CONVERSATION_HISTORY", "PLANNER_RESPONSE"]);

/** Pull the user's text out of `<USER_REQUEST>…</USER_REQUEST>`; fall back to
 *  the whole content if the wrapper is absent. */
function userText(content: string): string {
  const m = content.match(/<USER_REQUEST>\s*([\s\S]*?)\s*<\/USER_REQUEST>/);
  return (m ? m[1] : content).trim();
}

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  const pendingCallIds: string[] = []; // FIFO of unmatched tool-call ids

  for (const line of lines) {
    const step = asRecord(line);
    const type = typeof step.type === "string" ? step.type : "";
    const stepIndex = typeof step.step_index === "number" ? step.step_index : out.length;

    if (type === "USER_INPUT") {
      const content = typeof step.content === "string" ? step.content : "";
      out.push({ type: "user", text: userText(content) });
      continue;
    }
    if (type === "CONVERSATION_HISTORY") continue;

    if (type === "PLANNER_RESPONSE") {
      if (typeof step.content === "string" && step.content) {
        out.push({ type: "assistant", text: step.content });
      }
      const calls = Array.isArray(step.tool_calls) ? step.tool_calls : [];
      calls.forEach((raw, i) => {
        const tc = asRecord(raw);
        const id = `agy-${stepIndex}-${i}`;
        pendingCallIds.push(id);
        out.push({
          type: "tool_call",
          id,
          name: typeof tc.name === "string" ? tc.name : "tool",
          input: tc.args ?? {},
        });
      });
      continue;
    }

    // Any other step type with content is a tool result — pair it FIFO to the
    // oldest unmatched call.
    if (!FIXED.has(type) && step.content != null) {
      out.push({
        type: "tool_result",
        id: pendingCallIds.shift() ?? `agy-orphan-${stepIndex}`,
        content: typeof step.content === "string" ? step.content : step.content,
        is_error: step.status === "ERROR",
      });
    }
  }
  return out;
}
