import { Markdown } from "../../../Markdown";
import type { ToolPresenter } from "./types";
import { getStringField, renderToolResult, ToolBlock } from "./util";

/** Pull a metadata object off the input bag, if present and non-empty. */
function getMetadata(input: unknown): Record<string, unknown> | null {
  if (input && typeof input === "object" && "metadata" in input) {
    const v = (input as Record<string, unknown>).metadata;
    if (v && typeof v === "object" && Object.keys(v).length > 0) {
      return v as Record<string, unknown>;
    }
  }
  return null;
}

/** Extract the created task number from a result like
 *  "Task #4 created successfully: …". Returns "" when not found. */
function getTaskNumber(result: { content: unknown; is_error?: boolean } | null): string {
  if (!result || result.is_error) return "";
  const match = renderToolResult(result.content).match(/#(\d+)/);
  return match ? `#${match[1]}` : "";
}

export const taskCreatePresenter: ToolPresenter = {
  icon: "check",
  summary: (call, result) => {
    const subject = getStringField(call.input, "subject");
    const number = getTaskNumber(result);
    return (
      <>
        {number && <span style={{ color: "var(--fg-3)", marginRight: 8 }}>{number}</span>}
        {subject || "(untitled task)"}
      </>
    );
  },
  expanded: (call, result) => {
    const description = getStringField(call.input, "description");
    const activeForm = getStringField(call.input, "activeForm");
    const metadata = getMetadata(call.input);
    return (
      <>
        {description && (
          <blockquote
            style={{
              margin: "0 0 12px",
              padding: "4px 0 4px 14px",
              color: "var(--fg-2)",
              borderLeft: "2px solid var(--accent-line)",
              fontSize: "var(--fs-base)",
              lineHeight: 1.6,
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {description}
          </blockquote>
        )}
        {activeForm && (
          <div
            style={{
              marginBottom: metadata || result ? 12 : 0,
              color: "var(--fg-3)",
              fontSize: "var(--fs-base)",
            }}
          >
            ⟳ {activeForm}
          </div>
        )}
        {metadata && <ToolBlock label="metadata">{JSON.stringify(metadata, null, 2)}</ToolBlock>}
        {result && (
          <div
            className={result.is_error ? "m-agent" : "m-agent m-agent-dim"}
            style={{
              color: result.is_error ? "var(--danger)" : undefined,
              fontSize: "var(--fs-base)",
            }}
          >
            <Markdown>{renderToolResult(result.content)}</Markdown>
          </div>
        )}
      </>
    );
  },
};
