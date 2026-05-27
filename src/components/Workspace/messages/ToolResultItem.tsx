import { useState } from "react";
import type { ChatItem } from "../../../store";
import { firstLine } from "../../../util/format";

/** Companion to ToolUseItem — shows the tool's textual output,
 *  collapsed by default. Error results get a red tinge. */
export function ToolResultItem({
  item,
}: {
  item: Extract<ChatItem, { kind: "tool_result" }>;
}) {
  const [open, setOpen] = useState(false);
  const text = renderResult(item.content);
  return (
    <div>
      <button
        type="button"
        className="m-tool"
        onClick={() => setOpen((o) => !o)}
        style={{
          width: "100%",
          textAlign: "left",
          color: item.is_error ? "var(--danger)" : undefined,
        }}
      >
        <span className="t-name" style={{ color: item.is_error ? "var(--danger)" : undefined }}>
          ↳ result
        </span>
        <span className="t-arg">{firstLine(text, 120)}</span>
        <span className="t-result">{open ? "▾" : "▸"}</span>
      </button>
      {open && (
        <pre
          style={{
            margin: 0,
            padding: "8px 14px 12px",
            fontFamily: "var(--font-mono)",
            fontSize: 11.5,
            color: item.is_error ? "var(--danger)" : "var(--fg-2)",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {text}
        </pre>
      )}
    </div>
  );
}

function renderResult(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((block) => {
        if (block && typeof block === "object" && "text" in block) {
          return String((block as { text: unknown }).text ?? "");
        }
        return JSON.stringify(block);
      })
      .join("\n");
  }
  return JSON.stringify(content, null, 2);
}
