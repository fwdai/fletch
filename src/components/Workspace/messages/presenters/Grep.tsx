import type { ToolPresenter } from "./types";
import { countResultLines, getStringField, renderToolResult, SummaryNote, ToolBlock } from "./util";

/** A muted count of what the grep returned, labelled by output_mode:
 *  - files_with_matches (default): one path per line  -> "N files"
 *  - content:                      one match per line -> "N matches"
 *  - count:                        "path:N" lines      -> sum -> "N matches" */
function grepCount(
  call: { input: unknown },
  result: { content: unknown } | null,
): { count: number; label: string } {
  if (!result) return { count: 0, label: "" };
  const text = renderToolResult(result.content);
  if (/no matches found/i.test(text)) return { count: 0, label: "" };
  const mode = getStringField(call.input, "output_mode") || "files_with_matches";

  if (mode === "count") {
    let total = 0;
    for (const line of text.split("\n")) {
      const n = Number(line.slice(line.lastIndexOf(":") + 1).trim());
      if (Number.isFinite(n)) total += n;
    }
    return { count: total, label: total === 1 ? "match" : "matches" };
  }

  const lines = countResultLines(result);
  const label =
    mode === "content" ? (lines === 1 ? "match" : "matches") : lines === 1 ? "file" : "files";
  return { count: lines, label };
}

export const grepPresenter: ToolPresenter = {
  icon: "search",
  summary: (call, result) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    if (!pattern) return "(no pattern)";
    const text = path ? `${pattern}  in  ${path}` : pattern;
    const { count, label } = grepCount(call, result);
    return (
      <>
        {text}
        {count > 0 && (
          <SummaryNote>
            {count} {label}
          </SummaryNote>
        )}
      </>
    );
  },
  expanded: (call, result) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    const glob = getStringField(call.input, "glob");
    const type = getStringField(call.input, "type");
    return (
      <>
        <ToolBlock label="pattern">{pattern}</ToolBlock>
        {path && <ToolBlock label="path">{path}</ToolBlock>}
        {glob && <ToolBlock label="glob">{glob}</ToolBlock>}
        {type && <ToolBlock label="type">{type}</ToolBlock>}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
