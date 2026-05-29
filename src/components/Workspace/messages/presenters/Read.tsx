import type { ToolPresenter } from "./types";
import {
  ToolBlock,
  SummaryNote,
  countResultLines,
  getStringField,
  renderToolResult,
} from "./util";
import { basename } from "../../../../util/format";

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
        <ToolBlock label="file">{path}</ToolBlock>
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
