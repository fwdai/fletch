import type { ManagedItem } from "../../../store";
import { ToolUseItem } from "./ToolUseItem";
import { ToolResultItem } from "./ToolResultItem";

/** Dispatcher for a single message in the chat log. Each kind has its
 *  own visual treatment defined in app.css under `.m-*`. */
export function MessageItem({ item }: { item: ManagedItem }) {
  switch (item.kind) {
    case "user":
      return <div className="m-user">{item.text}</div>;
    case "assistant":
      return (
        <div className="m-agent">
          {item.text}
          {item.streaming && <span className="term-cursor" style={{ marginLeft: 4 }} />}
        </div>
      );
    case "tool_use":
      return <ToolUseItem item={item} />;
    case "tool_result":
      return <ToolResultItem item={item} />;
    case "system":
      return (
        <div className="m-reasoning">
          <div className="label">System</div>
          {item.text}
        </div>
      );
    case "result":
      return (
        <div
          className="m-reasoning"
          style={{
            color: item.is_error ? "var(--danger)" : "var(--fg-3)",
            borderLeftColor: item.is_error ? "var(--danger)" : undefined,
          }}
        >
          <div className="label">Result</div>
          {item.text}
        </div>
      );
  }
}
