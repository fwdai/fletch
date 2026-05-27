import type { ToolPresenter } from "./types";
import { ToolBlock, getStringField, renderToolResult } from "./util";
import { basename } from "../../../../util/format";

export const readPresenter: ToolPresenter = {
  summary: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? basename(path) : "(no path)";
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
