import type { ToolPresenter } from "./types";
import { ToolBlock, getStringField, renderToolResult } from "./util";
import { basename } from "../../../../util/format";

export const writePresenter: ToolPresenter = {
  icon: "file",
  summary: (call) => {
    const path = getStringField(call.input, "file_path");
    return path ? basename(path) : "(no path)";
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
