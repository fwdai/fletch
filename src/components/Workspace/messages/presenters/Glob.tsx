import type { ToolPresenter } from "./types";
import {
  countResultLines,
  firstLineOf,
  getStringField,
  renderToolResult,
  SummaryNote,
  ToolBlock,
} from "./util";

/** Glob emits one matched path per line, or a "No files found" sentinel. */
function matchCount(result: { content: unknown } | null): number {
  if (!result) return 0;
  if (/no files found/i.test(renderToolResult(result.content))) return 0;
  return countResultLines(result);
}

export const globPresenter: ToolPresenter = {
  icon: "search",
  summary: (call, result) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    if (!pattern) return "(no pattern)";
    const text = path ? `${pattern}  in  ${path}` : pattern;
    const count = matchCount(result);
    return (
      <>
        {firstLineOf(text, 140)}
        {count > 0 && (
          <SummaryNote>
            {count} {count === 1 ? "file" : "files"}
          </SummaryNote>
        )}
      </>
    );
  },
  expanded: (call, result) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    return (
      <>
        <ToolBlock label="$">{pattern}</ToolBlock>
        {path && <ToolBlock label="path">{path}</ToolBlock>}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
