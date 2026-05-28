import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatItem } from "../../../store";
import type { ViewItem } from "./pair";
import { ToolResultItem } from "./ToolResultItem";
import { ToolRow } from "./ToolRow";
import { getPresenter } from "./presenters";

/** Dispatcher for one rendered row. Accepts either a raw ChatItem or
 *  the derived `tool_pair` from pairToolItems(). */
export function MessageItem({ item }: { item: ViewItem }) {
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
    case "tool_pair": {
      const presenter = getPresenter(item.call.name);
      return (
        <ToolRow
          name={item.call.name}
          icon={presenter.icon}
          isError={item.result?.is_error}
          summary={presenter.summary(item.call, item.result)}
          expanded={presenter.expanded(item.call, item.result)}
        />
      );
    }
    case "tool_call": {
      // Bare tool_call without pairing — happens only if the caller
      // bypasses pairToolItems(). Render through the presenter anyway,
      // with a null result.
      const presenter = getPresenter(item.name);
      return (
        <ToolRow
          name={item.name}
          icon={presenter.icon}
          summary={presenter.summary(item, null)}
          expanded={presenter.expanded(item, null)}
        />
      );
    }
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
  if (item.subtype === "compact_summary") {
    return (
      <div className="m-reasoning" style={{ fontStyle: "italic" }}>
        <div className="label">system</div>
        {item.text}
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
