import { basename } from "@/util/format";
import { lineDiffCounts } from "@/util/lineDiff";
import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { DiffCount, getStringField } from "./util";

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
  facts: (call) => ({
    kind: "change",
    path: getStringField(call.input, "file_path"),
    ...lineDiffCounts("", getStringField(call.input, "content")),
  }),
  activity: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? `Writing ${basename(path)}` : "Writing a file";
  },
};
