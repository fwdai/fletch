import type { ToolPresenter } from "./types";
import { firstLineOf, getCommandField, getStringField, renderToolResult, ToolBlock } from "./util";

export const bashPresenter: ToolPresenter = {
  icon: "terminal",
  summary: (call) => {
    const cmd = getCommandField(call.input);
    return cmd ? firstLineOf(cmd, 140) : "(no command)";
  },
  expanded: (call, result) => {
    const cmd = getCommandField(call.input);
    const description = getStringField(call.input, "description");
    return (
      <>
        {description && <ToolBlock label="·">{description}</ToolBlock>}
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
