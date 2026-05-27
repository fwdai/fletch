import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatItem } from "../../../store";
import { ToolUseItem } from "./ToolUseItem";
import { ToolResultItem } from "./ToolResultItem";

/** Dispatcher for a single normalized message in the chat log. Each kind
 *  has its own visual treatment defined in app.css under `.m-*`. */
export function MessageItem({ item }: { item: ChatItem }) {
  switch (item.kind) {
    case "user_message":
      return <div className="m-user">{item.text}</div>;
    case "agent_message":
      return (
        <div className="m-agent">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{item.text}</ReactMarkdown>
          {item.streaming && (
            <span className="term-cursor" style={{ marginLeft: 4 }} />
          )}
        </div>
      );
    case "tool_call":
      return <ToolUseItem item={item} />;
    case "tool_result":
      return <ToolResultItem item={item} />;
    case "notice":
      return <NoticeView item={item} />;
  }
}

function NoticeView({
  item,
}: {
  item: Extract<ChatItem, { kind: "notice" }>;
}) {
  if (item.subtype === "slash_command") {
    return (
      <div className="m-reasoning" style={{ fontStyle: "italic" }}>
        <div className="label">command</div>
        <code>{item.text}</code>
      </div>
    );
  }
  const isError = item.is_error || item.subtype === "error";
  return (
    <div
      className="m-reasoning"
      style={{
        color: isError ? "var(--danger)" : "var(--fg-3)",
        borderLeftColor: isError ? "var(--danger)" : undefined,
      }}
    >
      <div className="label">{labelFor(item.subtype)}</div>
      {item.text}
    </div>
  );
}

function labelFor(subtype: string): string {
  switch (subtype) {
    case "error":
      return "Error";
    case "reasoning":
      return "Thinking";
    case "hook_output":
      return "Hook";
    case "turn_end":
      return "Turn";
    case "info":
      return "Info";
    default:
      return subtype;
  }
}
