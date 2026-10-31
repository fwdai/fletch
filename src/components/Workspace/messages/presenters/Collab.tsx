import { renderToolResult } from "@/util/toolText";
import { CodeSurface, Fields } from "./blocks";
import type { ToolPresenter } from "./types";

/** Codex's multi-agent coordination calls (`collab.<tool>`, see the codex
 *  reducer): what the parent does *about* its sub-agents — wait for them, send
 *  one input, close or interrupt one. The sub-agents' own turns nest under the
 *  spawning `Agent` row, not here. */
const SUMMARIES: Record<string, string> = {
  wait: "Waiting for sub-agents",
  send_input: "Sending to sub-agent",
  close: "Closing sub-agent",
  interrupt: "Interrupting sub-agent",
  list: "Listing sub-agents",
};

function toolOf(name: string): string {
  return name.replace(/^collab\./, "");
}

function summaryOf(name: string): string {
  const tool = toolOf(name);
  return SUMMARIES[tool] ?? `Sub-agent ${tool.replace(/_/g, " ")}`;
}

export const collabPresenter: ToolPresenter = {
  icon: "bot",
  title: "Sub-agents",
  summary: (call) => summaryOf(call.name),
  expanded: (call, result) => {
    const text = result ? renderToolResult(result.content) : "";
    return (
      <>
        <Fields input={call.input} />
        {text && <CodeSurface text={text} isError={result?.is_error} />}
      </>
    );
  },
  facts: (call) => ({ kind: "other", subject: summaryOf(call.name).toLowerCase() }),
  activity: (call) => summaryOf(call.name),
};
