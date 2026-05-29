import type { ToolPresenter } from "./types";
import {
  ToolBlock,
  SummaryNote,
  getStringField,
  renderToolResult,
} from "./util";
import { basename } from "../../../../util/format";

/** Count the lines of content actually returned by the read. */
function countLines(result: { content: unknown } | null): number {
  if (!result) return 0;
  const text = renderToolResult(result.content).replace(/\n$/, "");
  return text === "" ? 0 : text.split("\n").length;
}

export const readPresenter: ToolPresenter = {
  icon: "file",
  summary: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const lines = countLines(result);
    return (
      <>
        {path ? basename(path) : "(no path)"}
        {lines > 0 && (
          <SummaryNote>
            {lines} {lines === 1 ? "line" : "lines"}
          </SummaryNote>
        )}
      </>
    );
  },
  expanded: (call, result) => {
    const path = getStringField(call.input, "file_path");
    return (
      <>
        <ToolBlock label="file">{path}</ToolBlock>
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
