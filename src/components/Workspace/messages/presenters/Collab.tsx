import type { ToolPresenter } from "./types";
import { renderToolResult, stringifyInput, ToolBlock } from "./util";

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

export const collabPresenter: ToolPresenter = {
  icon: "bot",
  title: "Sub-agents",
  summary: (call) => {
    const tool = toolOf(call.name);
    return SUMMARIES[tool] ?? `Sub-agent ${tool.replace(/_/g, " ")}`;
  },
  expanded: (call, result) => {
    const input = stringifyInput(call.input, 2);
    return (
      <>
        {input && input !== "{}" && <ToolBlock label="input">{input}</ToolBlock>}
        {result && renderToolResult(result.content) && (
          <ToolBlock label="result" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
