// Serialization between rendered chat items and the `messages` table —
// the provider-agnostic history store. `messageRowFor` and
// `messagesToChatItems` are inverses; keep them in sync.

import type { ChatItem, NoticeSubtype } from "../adapters";
import type { MessageRow } from "./messages";

export function safeParse(s: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    return s;
  }
}

export type NewMessageRow = Omit<MessageRow, "id" | "created_at">;

/** Serialize one chat item into a messages-table row. */
export function messageRowFor(
  item: ChatItem,
  sequence: number,
  agentId: string,
): NewMessageRow {
  const content =
    item.kind === "user_message" ||
    item.kind === "agent_message" ||
    item.kind === "notice"
      ? item.text
      : JSON.stringify(
          "input" in item ? item.input : "content" in item ? item.content : null,
        );
  const metadata_json =
    item.kind === "tool_call"
      ? JSON.stringify({ name: item.name, id: item.id })
      : item.kind === "tool_result"
        ? JSON.stringify({ tool_use_id: item.tool_use_id, is_error: item.is_error })
        : item.kind === "notice"
          ? JSON.stringify({ subtype: item.subtype })
          : null;
  return { agent_id: agentId, kind: item.kind, content: content || "", metadata_json, sequence };
}

/** Inverse of `messageRowFor` — rebuild chat items from persisted rows. */
export function messagesToChatItems(rows: MessageRow[]): ChatItem[] {
  const out: ChatItem[] = [];
  for (const r of rows) {
    const meta = r.metadata_json
      ? (safeParse(r.metadata_json) as Record<string, unknown>)
      : {};
    switch (r.kind) {
      case "user_message":
        out.push({ kind: "user_message", text: r.content });
        break;
      case "agent_message":
        out.push({ kind: "agent_message", text: r.content });
        break;
      case "notice":
        out.push({
          kind: "notice",
          subtype: (meta.subtype as NoticeSubtype) ?? "info",
          text: r.content,
        });
        break;
      case "tool_call":
        out.push({
          kind: "tool_call",
          id: String(meta.id ?? ""),
          name: String(meta.name ?? "tool"),
          input: safeParse(r.content),
        });
        break;
      case "tool_result":
        out.push({
          kind: "tool_result",
          tool_use_id: String(meta.tool_use_id ?? ""),
          content: safeParse(r.content),
          is_error: meta.is_error === true,
        });
        break;
    }
  }
  return out;
}
