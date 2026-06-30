import { basename } from "@/util/format";
import { lineDiffCounts } from "@/util/lineDiff";
import type { ToolPresenter } from "./types";
import { DiffCount, getStringField, renderToolResult, ToolBlock } from "./util";

export const writePresenter: ToolPresenter = {
  icon: "notebookPen",
  summary: (call) => {
    const path = getStringField(call.input, "file_path");
    const content = getStringField(call.input, "content");
    // Write replaces the whole file; the prior version isn't in the call, so
    // every line counts as an addition.
    const { additions, deletions } = lineDiffCounts("", content);
    return (
      <>
        {path ? basename(path) : "(no path)"}
        <DiffCount additions={additions} deletions={deletions} />
      </>
    );
  },
  expanded: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const content = getStringField(call.input, "content");
    return (
      <>
        <ToolBlock label="file">{path}</ToolBlock>
        {content && <ToolBlock label="body">{content}</ToolBlock>}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
