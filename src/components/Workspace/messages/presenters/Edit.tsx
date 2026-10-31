import { basename } from "@/util/format";
import { lineDiffCounts } from "@/util/lineDiff";
import { renderToolResult } from "@/util/toolText";
import { Caption, DiffSurface, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { DiffCount, getStringField } from "./util";

export const editPresenter: ToolPresenter = {
  icon: "edit",
  summary: (call) => {
    const path = getStringField(call.input, "file_path");
    const oldStr = getStringField(call.input, "old_string");
    const newStr = getStringField(call.input, "new_string");
    const { additions, deletions } = lineDiffCounts(oldStr, newStr);
    return (
      <>
        {path ? basename(path) : "(no path)"}
        <DiffCount additions={additions} deletions={deletions} />
      </>
    );
  },
  expanded: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const oldStr = getStringField(call.input, "old_string");
    const newStr = getStringField(call.input, "new_string");
    return (
      <>
        {path && (
          <Caption>
            <PathText path={path} />
          </Caption>
        )}
        <DiffSurface before={oldStr} after={newStr} />
        {/* A successful edit's result only restates the path; a failed one
            says why, and that's worth the space. */}
        {result?.is_error && <Caption tone="danger">{renderToolResult(result.content)}</Caption>}
      </>
    );
  },
  facts: (call) => {
    const { additions, deletions } = lineDiffCounts(
      getStringField(call.input, "old_string"),
      getStringField(call.input, "new_string"),
    );
    return { kind: "change", path: getStringField(call.input, "file_path"), additions, deletions };
  },
  activity: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? `Editing ${basename(path)}` : "Editing a file";
  },
};
