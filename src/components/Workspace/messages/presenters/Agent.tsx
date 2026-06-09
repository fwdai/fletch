import { Markdown } from "../../../Markdown";
import type { ToolPresenter } from "./types";
import { getStringField, renderToolResult } from "./util";

export const agentPresenter: ToolPresenter = {
  icon: "zap",
  summary: (call) => {
    const subagentType = getStringField(call.input, "subagent_type");
    const description = getStringField(call.input, "description");
    return (
      <>
        {subagentType && (
          <span style={{ color: "var(--fg-3)", marginRight: 8 }}>
            {subagentType}
          </span>
        )}
        {description}
      </>
    );
  },
  expanded: (call, result) => {
    const prompt = getStringField(call.input, "prompt");
    return (
      <>
        {prompt && (
          <blockquote
            style={{
              margin: "0 0 12px",
              padding: "4px 0 4px 14px",
              color: "var(--fg-2)",
              borderLeft: "2px solid var(--accent-line)",
              fontSize: 13.5,
              lineHeight: 1.6,
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {prompt}
          </blockquote>
        )}
        {result && (
          <div
            className={
              result.is_error ? "m-agent" : "m-agent m-agent-dim"
            }
            style={{
              color: result.is_error ? "var(--danger)" : undefined,
              fontSize: 13.5,
            }}
          >
            <Markdown>{renderToolResult(result.content)}</Markdown>
          </div>
        )}
      </>
    );
  },
};
