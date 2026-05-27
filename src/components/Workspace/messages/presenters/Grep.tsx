import type { ToolPresenter } from "./types";
import { ToolBlock, getStringField, renderToolResult } from "./util";

export const grepPresenter: ToolPresenter = {
  icon: "search",
  summary: (call) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    if (!pattern) return "(no pattern)";
    return path ? `${pattern}  in  ${path}` : pattern;
  },
  expanded: (call, result) => {
    const pattern = getStringField(call.input, "pattern");
    const path = getStringField(call.input, "path");
    const glob = getStringField(call.input, "glob");
    const type = getStringField(call.input, "type");
    return (
      <>
        <ToolBlock label="pattern">{pattern}</ToolBlock>
        {path && <ToolBlock label="path">{path}</ToolBlock>}
        {glob && <ToolBlock label="glob">{glob}</ToolBlock>}
        {type && <ToolBlock label="type">{type}</ToolBlock>}
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
