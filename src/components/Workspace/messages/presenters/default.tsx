import { renderToolResult } from "@/util/toolText";
import { CodeSurface, Fields } from "./blocks";
import type { ToolPresenter } from "./types";
import { describeInput } from "./util";

/** Fallback presenter for tools without a dedicated implementation:
 *  a readable one-line rendering of the input when collapsed; opened, the
 *  input as labelled fields and the result on a code surface. */
export const defaultPresenter: ToolPresenter = {
  summary: (call) => describeInput(call.input, 120),
  expanded: (call, result) => {
    const text = result ? renderToolResult(result.content) : "";
    return (
      <>
        <Fields input={call.input} />
        {result && <CodeSurface text={text} isError={result.is_error} />}
      </>
    );
  },
};
