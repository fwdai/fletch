// Reducer for Pi's `pi -p --mode json` event stream.
//
// Verified against pi 0.74.2 (@earendil-works/pi-coding-agent). Pi emits its
// OWN event schema — not Claude-, Codex-, or OpenCode-shaped — as
// newline-delimited JSON. The lifecycle of one turn:
//
//   {"type":"session","id":"<uuid>","cwd":"…"}                 // session id (captured in Rust)
//   {"type":"agent_start"} / {"type":"turn_start"}
//   {"type":"message_start"|"message_update"|"message_end", "message":{role,content}}
//   {"type":"tool_execution_start"|"_update"|"_end", "toolCallId","toolName","result","isError"}
//   {"type":"turn_end", …}                                     // fires PER assistant step
//   {"type":"agent_end", …}                                    // once, at the end of the turn
//
// We reduce off the *finalized* events: `message_end` carries whole messages
// (role user / assistant / toolResult) with Anthropic-style content blocks
// ({type:"text"} and {type:"toolCall",id,name,arguments}), and
// `tool_execution_end` carries the tool result. The token-level deltas
// (`message_update` with text_/toolcall_ sub-events) are skipped — live
// streaming is a follow-up. `agent_end` is the end-of-turn marker (NOT
// `turn_end`, which fires once per assistant step).

import type { ChatItem, RawEvent } from "../types";
import { asBlockList, asRecord } from "../shared/json";
import {
  dedupAgainstLast,
  finalizeStreamingItems,
  upsertToolCall,
} from "../shared/reducer-helpers";

/** Pi names file tools' path argument `path`; the shared presenters read
 *  Claude's `file_path`. Add the alias so Read/Write/Edit render the path.
 *  Returns the input untouched when there's nothing to alias (e.g. bash's
 *  `command`). */
function normalizeToolInput(input: unknown): unknown {
  const rec = asRecord(input);
  if (typeof rec.path === "string" && rec.file_path === undefined) {
    return { ...rec, file_path: rec.path };
  }
  return input;
}

/** Flatten a content-block array (`[{type:"text",text}]`) to a string. */
function textOfBlocks(content: unknown): string {
  return asBlockList(content)
    .filter((b) => b.type === "text")
    .map((b) => (typeof b.text === "string" ? b.text : ""))
    .join("");
}

export function reduce(prev: ChatItem[], ev: RawEvent): ChatItem[] {
  const type = typeof ev.type === "string" ? ev.type : undefined;

  switch (type) {
    case "message_end": {
      const msg = asRecord(ev.message);
      const role = typeof msg.role === "string" ? msg.role : "";

      if (role === "user") {
        const text = textOfBlocks(msg.content);
        if (!text) return prev;
        return dedupAgainstLast(prev, { kind: "user_message", text });
      }

      if (role === "assistant") {
        let items = prev;
        for (const block of asBlockList(msg.content)) {
          if (block.type === "text") {
            const text = typeof block.text === "string" ? block.text : "";
            if (text) items = dedupAgainstLast(items, { kind: "agent_message", text });
          } else if (block.type === "toolCall") {
            const id = typeof block.id === "string" ? block.id : "";
            if (!id) continue;
            items = upsertToolCall(items, {
              kind: "tool_call",
              id,
              name: typeof block.name === "string" ? block.name : "tool",
              input: normalizeToolInput(block.arguments),
              streaming: false,
            });
          }
        }
        return items;
      }

      // role === "toolResult": the result is rendered from
      // `tool_execution_end` instead, so skip the duplicate message.
      return prev;
    }

    case "tool_execution_end": {
      const id = typeof ev.toolCallId === "string" ? ev.toolCallId : "";
      if (!id) return prev;
      const result = asRecord(ev.result);
      return [
        ...prev,
        {
          kind: "tool_result",
          tool_use_id: id,
          content: textOfBlocks(result.content),
          is_error: ev.isError === true,
        },
      ];
    }

    // The whole turn is done (fires once; `turn_end` fires per step).
    case "agent_end": {
      const items = finalizeStreamingItems(prev);
      return [...items, { kind: "notice", subtype: "turn_end", text: "success" }];
    }

    default:
      // session / agent_start / turn_start / turn_end / message_start /
      // message_update (streaming deltas) carry nothing we render whole.
      return prev;
  }
}
