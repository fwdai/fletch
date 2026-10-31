import { renderToolResult } from "@/util/toolText";
import { Caption, TermSurface } from "./blocks";
import type { ToolPresenter } from "./types";
import { commandHead, firstLineOf, getCommandField, getStringField, progressive } from "./util";

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
        {description && <Caption>{description}</Caption>}
        <TermSurface
          command={cmd}
          output={result ? renderToolResult(result.content) : null}
          isError={result?.is_error}
        />
      </>
    );
  },
  // The summary names the command itself: "ran cargo test" reads; a
  // description ("Run the tests") would read "ran Run the tests".
  facts: (call) => ({ kind: "run", subject: commandHead(getCommandField(call.input)) }),
  // The agent's own description says what the command is *for* ("Running the
  // unit tests"); without a description we know, name the command itself.
  activity: (call) => {
    const said = progressive(getStringField(call.input, "description"));
    if (said) return said;
    const head = commandHead(getCommandField(call.input));
    return head ? `Running ${head}` : "Running a command";
  },
};
