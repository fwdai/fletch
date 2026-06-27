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

import { asBlockList, asRecord } from "../shared/json";
import {
  aliasToolInput,
  dedupAgainstLast,
  endTurn,
  finalizeStreamingItems,
  upsertToolCall,
} from "../shared/reducer-helpers";
import type { ChatItem, RawEvent } from "../types";

/** Pi names file tools' path argument `path`; the shared presenters read
 *  Claude's `file_path`. Alias it so Read/Write/Edit render the path. */
function normalizeToolInput(input: unknown): unknown {
  return aliasToolInput(input, [["path", "file_path"]]);
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
        // pi reports the model on every message object; stamp it onto the
        // turn's agent_message so the UI can show the actual model in use.
        const model = typeof msg.model === "string" ? msg.model : undefined;
        let items = prev;
        for (const block of asBlockList(msg.content)) {
          if (block.type === "thinking") {
            // Extended-thinking block (`pi --thinking`, verified against
            // 0.78.0): {type:"thinking",thinking:"…",thinkingSignature:"…"}.
            // Surface the text as a reasoning notice.
            const text = typeof block.thinking === "string" ? block.thinking : "";
            if (text) items = [...items, { kind: "notice", subtype: "reasoning", text }];
          } else if (block.type === "text") {
            const text = typeof block.text === "string" ? block.text : "";
            if (text)
              items = dedupAgainstLast(items, {
                kind: "agent_message",
                text,
                model,
              });
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
      return endTurn(finalizeStreamingItems(prev));
    }

    default:
      // session / agent_start / turn_start / turn_end / message_start /
      // message_update (streaming deltas) carry nothing we render whole.
      return prev;
  }
}
