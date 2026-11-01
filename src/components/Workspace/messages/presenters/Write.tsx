import { basename } from "@/util/format";
import { lineDiffCounts } from "@/util/lineDiff";
import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface, PathText } from "./blocks";
import type { ToolCall, ToolPresenter, ToolResult } from "./types";
import { DiffCount, getStringField } from "./util";

/** What a Write did, read off Claude's result: "File created successfully at:
 *  …" for a new file, "The file … has been updated successfully." for an
 *  overwrite. Only a new file has countable lines — the call carries the new
 *  contents and not the old, so an overwrite's additions and deletions are
 *  unknown and are left unsaid rather than made up. */
function lineCounts(
  call: ToolCall,
  result: ToolResult | null,
): { additions: number; deletions: number } | null {
  if (!result || result.is_error) return null;
  if (!/\bcreated\b/i.test(renderToolResult(result.content))) return null;
  return lineDiffCounts("", getStringField(call.input, "content"));
}

export const writePresenter: ToolPresenter = {
  icon: "notebookPen",
  summary: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const counts = lineCounts(call, result);
    return (
      <>
        {path ? basename(path) : "(no path)"}
        {counts && <DiffCount additions={counts.additions} deletions={counts.deletions} />}
      </>
    );
  },
  expanded: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const content = getStringField(call.input, "content");
    return (
      <>
        {path && (
          <Caption>
            <PathText path={path} />
          </Caption>
        )}
        {content && <CodeSurface text={content} />}
        {result?.is_error && <Caption tone="danger">{renderToolResult(result.content)}</Caption>}
      </>
    );
  },
  facts: (call, result) => ({
    kind: "change",
    path: getStringField(call.input, "file_path"),
    ...lineCounts(call, result),
  }),
  activity: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? `Writing ${basename(path)}` : "Writing a file";
  },
};
