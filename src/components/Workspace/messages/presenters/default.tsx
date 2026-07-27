import type { ToolPresenter } from "./types";
import { describeInput, renderToolResult, stringifyInput, ToolBlock } from "./util";

/** Fallback presenter for tools without a dedicated implementation:
 *  a readable one-line rendering of the input when collapsed, the exact
 *  input JSON plus the result when expanded. */
export const defaultPresenter: ToolPresenter = {
  summary: (call) => describeInput(call.input, 120),
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
