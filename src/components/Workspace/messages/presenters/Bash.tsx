import type { ToolPresenter } from "./types";
import {
  ToolBlock,
  firstLineOf,
  getStringField,
  renderToolResult,
} from "./util";

export const bashPresenter: ToolPresenter = {
  summary: (call) => {
    const cmd = getStringField(call.input, "command");
    return cmd ? firstLineOf(cmd, 140) : "(no command)";
  },
  expanded: (call, result) => {
    const cmd = getStringField(call.input, "command");
    const description = getStringField(call.input, "description");
    return (
      <>
        {description && (
          <ToolBlock label="·">{description}</ToolBlock>
        )}
        <ToolBlock label="$">{cmd}</ToolBlock>
        {result && (
          <ToolBlock label="↳" isError={result.is_error}>
            {renderToolResult(result.content)}
          </ToolBlock>
        )}
      </>
    );
  },
};
