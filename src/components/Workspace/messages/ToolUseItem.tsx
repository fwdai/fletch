import { useState } from "react";
import type { ChatItem } from "../../../store";
import { Icon } from "../../Icon";
import { firstLine } from "../../../util/format";

/** Single tool-call row. Defaults to a one-line summary; click to
 *  expand the full input JSON. */
export function ToolUseItem({
  item,
}: {
  item: Extract<ChatItem, { kind: "tool_call" }>;
}) {
  const [open, setOpen] = useState(false);
  const summary = summarize(item.input);
  return (
    <div>
      <button
        type="button"
        className="m-tool"
        onClick={() => setOpen((o) => !o)}
        style={{ width: "100%", textAlign: "left" }}
      >
        <Icon name="wrench" size={12} className="t-icon" />
        <span className="t-name">{item.name}</span>
        <span className="t-arg">{summary}</span>
        <span className="t-result">{open ? "▾" : "▸"}</span>
      </button>
      {open && (
        <pre
          style={{
            margin: 0,
            padding: "8px 14px 12px",
            fontFamily: "var(--font-mono)",
            fontSize: 11.5,
            color: "var(--fg-2)",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {JSON.stringify(item.input, null, 2)}
        </pre>
      )}
    </div>
  );
}

function summarize(input: unknown): string {
  if (input == null) return "";
  if (typeof input === "string") return firstLine(input, 120);
  try {
    return firstLine(JSON.stringify(input), 120);
  } catch {
    return "";
  }
}
