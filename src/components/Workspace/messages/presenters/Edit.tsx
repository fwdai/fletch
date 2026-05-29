import type { ToolPresenter } from "./types";
import { ToolBlock, DiffCount, getStringField, renderToolResult } from "./util";
import { basename } from "../../../../util/format";
import { lineDiffCounts } from "../../../../util/lineDiff";

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
        <ToolBlock label="file">{path}</ToolBlock>
        {oldStr && (
          <ToolBlock label="- old" isError>
            {oldStr}
          </ToolBlock>
        )}
        {newStr && (
          <ToolBlock label="+ new">
            <span style={{ color: "var(--success, #6c9c5e)" }}>{newStr}</span>
          </ToolBlock>
        )}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
