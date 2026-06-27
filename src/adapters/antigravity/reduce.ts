// Reducer for Antigravity. normalizeTranscript pre-digests agy's on-disk steps
// into a small canonical vocabulary (user / assistant / tool_call / tool_result),
// so this is a thin mapper to ChatItems. There is no live JSON stream for agy
// (its turn runner is plaintext); the structured render comes entirely from the
// transcript replayed through here.

import { upsertToolCall } from "../shared/reducer-helpers";
import type { ChatItem, RawEvent } from "../types";

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  switch (ev.type) {
    case "user": {
      const text = typeof ev.text === "string" ? ev.text : "";
      return text ? [...prev, { kind: "user_message", text }] : prev;
    }
    case "assistant": {
      const text = typeof ev.text === "string" ? ev.text : "";
      return text ? [...prev, { kind: "agent_message", text }] : prev;
    }
    case "tool_call": {
      const id = typeof ev.id === "string" ? ev.id : "";
      if (!id) return prev;
      return upsertToolCall(prev, {
        kind: "tool_call",
        id,
        name: typeof ev.name === "string" ? ev.name : "tool",
        input: ev.input ?? {},
        streaming: false,
      });
    }
    case "tool_result": {
      const id = typeof ev.id === "string" ? ev.id : "";
      if (!id) return prev;
      return [
        ...prev,
        {
          kind: "tool_result",
          tool_use_id: id,
          content: ev.content ?? "",
          is_error: ev.is_error === true,
        },
      ];
    }
    default:
      return prev;
  }
}
