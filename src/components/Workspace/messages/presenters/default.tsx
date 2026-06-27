import type { ToolPresenter } from "./types";
import { firstLineOf, renderToolResult, stringifyInput, ToolBlock } from "./util";

/** Fallback presenter for tools without a dedicated implementation.
 *  Mirrors the prior generic ToolUseItem/ToolResultItem behavior:
 *  one-line JSON-summary in collapsed view, full input + result when
 *  expanded. */
export const defaultPresenter: ToolPresenter = {
  summary: (call) => firstLineOf(stringifyInput(call.input), 120),
  expanded: (call, result) => (
    <>
      <ToolBlock label="input">{stringifyInput(call.input, 2)}</ToolBlock>
      {result && (
        <ToolBlock label="result" isError={result.is_error}>
          {renderToolResult(result.content)}
        </ToolBlock>
      )}
    </>
  ),
};
