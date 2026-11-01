import { basename } from "@/util/format";
import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { countResultLines, getStringField, SummaryNote } from "./util";

export const readPresenter: ToolPresenter = {
  icon: "file",
  summary: (call, result) => {
    const path = getStringField(call.input, "file_path");
    const lines = countResultLines(result);
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
        {path && (
          <Caption>
            <PathText path={path} />
          </Caption>
        )}
        {result && (
          <CodeSurface text={renderToolResult(result.content)} isError={result.is_error} />
        )}
      </>
    );
  },
  facts: (call) => ({ kind: "read", path: getStringField(call.input, "file_path") }),
  activity: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? `Reading ${basename(path)}` : "Reading a file";
  },
};
