import { Icon } from "@desktop/components/Icon";
import { useState } from "react";
import type { ChatItem } from "../../adapters";
import { resultSummary, resultText, TOOL_HUE, TOOL_ICON, toolArg } from "../../lib/tools";

type ToolCall = Extract<ChatItem, { kind: "tool_call" }>;
type ToolResult = Extract<ChatItem, { kind: "tool_result" }>;

export function ToolRow({ call, result }: { call: ToolCall; result: ToolResult | null }) {
  const [open, setOpen] = useState(false);
  const out = result ? resultText(result.content) : "";
  const tone = TOOL_HUE[call.name] ?? "var(--fg-2)";
  return (
    <div className="tool-wrap">
      <button
        type="button"
        className={`tool${open ? " open" : ""}`}
        style={{ "--tool": tone } as React.CSSProperties}
        onClick={() => out && setOpen((o) => !o)}
      >
        <span className="ic">
          <Icon name={TOOL_ICON[call.name] ?? "wrench"} size={13} />
        </span>
        <span className="nm">{call.name}</span>
        <span className="arg">{toolArg(call.input)}</span>
        {call.streaming ? (
          <span className="mt run">
            <span className="working-dots">
              <i />
              <i />
              <i />
            </span>
          </span>
        ) : (
          out && <span className={`mt${result?.is_error ? " err" : ""}`}>{resultSummary(out)}</span>
        )}
        {out && <Icon name="chevR" size={14} className="chev" />}
      </button>
      {open && out && <div className="tool-out">{out}</div>}
    </div>
  );
}
