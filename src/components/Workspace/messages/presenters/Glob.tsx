import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { countResultLines, firstLineOf, getStringField, SummaryNote } from "./util";

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
    const path = getStringField(call.input, "path");
    const count = matchCount(result);
    return (
      <>
        <Caption>
          <span className="tb-mono">{getStringField(call.input, "pattern")}</span>
          {path && (
            <>
              <span className="tb-cap-sep">in</span>
              <PathText path={path} />
            </>
          )}
          {result && (
            <span className="tb-cap-note">
              {count > 0 ? `${count} ${count === 1 ? "file" : "files"}` : "no files"}
            </span>
          )}
        </Caption>
        {result && count > 0 && (
          <CodeSurface text={renderToolResult(result.content)} isError={result.is_error} />
        )}
      </>
    );
  },
  facts: (call) => ({ kind: "search", subject: getStringField(call.input, "pattern") }),
  activity: (call) => {
    const pattern = getStringField(call.input, "pattern");
    return pattern ? `Finding ${firstLineOf(pattern, 48)}` : "Finding files";
  },
};
