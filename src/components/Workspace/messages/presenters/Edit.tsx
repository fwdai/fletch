import type { ToolPresenter } from "./types";
import { ToolBlock, getStringField, renderToolResult } from "./util";
import { basename } from "../../../../util/format";

export const editPresenter: ToolPresenter = {
  icon: "edit",
  summary: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? basename(path) : "(no path)";
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
