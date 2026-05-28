import type { ToolPresenter } from "./types";
import {
  ToolBlock,
  firstLineOf,
  getStringField,
  renderToolResult,
} from "./util";

export const globPresenter: ToolPresenter = {
  icon: "search",
  summary: (call) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    if (!pattern) return "(no pattern)";
    const text = path ? `${pattern}  in  ${path}` : pattern;
    return firstLineOf(text, 140);
  },
  expanded: (call, result) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    return (
      <>
        <ToolBlock label="$">{pattern}</ToolBlock>
        {path && <ToolBlock label="path">{path}</ToolBlock>}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
