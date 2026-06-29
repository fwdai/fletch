import { describe, expect, it } from "vitest";
import type { ChatItem } from "../../../adapters";
import { APP_ACTION_PREFIX } from "../../RightPanel/delegation";
import { failedTurnIndex } from "./failedTurn";
import type { ViewItem } from "./pair";

const user = (text: string, failed?: boolean): ChatItem => ({
  kind: "user_message",
  text,
  ...(failed ? { failed: true } : {}),
});
const agent = (text: string): ChatItem => ({ kind: "agent_message", text });
const errorNotice = (): ChatItem => ({ kind: "notice", subtype: "error", text: "boom" });
const turnEnd = (): ChatItem => ({ kind: "notice", subtype: "turn_end", text: "" });
const idle = { busy: false, loading: false };

function run(items: ViewItem[], opts = idle) {
  return failedTurnIndex(items, opts);
}

describe("failedTurnIndex", () => {
  it("returns -1 for a healthy completed turn", () => {
    expect(run([user("hi"), agent("hello"), turnEnd()])).toBe(-1);
  });

  it("flags a send that threw (Case A) via the failed marker", () => {
    expect(run([user("first"), agent("ok"), user("oops", true)])).toBe(2);
  });

  it("flags the latest user turn when its response errored (Case B)", () => {
    expect(run([user("do a thing"), errorNotice()])).toBe(0);
  });

  it("flags Case B even when tool rows sit between the prompt and the error", () => {
    const toolPair: ViewItem = {
      kind: "tool_pair",
      // biome-ignore lint/suspicious/noExplicitAny: minimal stub for the test
      call: { id: "t1", name: "Bash", input: {} } as any,
      result: null,
    };
    expect(run([user("run it"), toolPair, errorNotice()])).toBe(0);
  });

  it("prefers the flagged send over an earlier errored turn", () => {
    expect(run([user("a"), errorNotice(), user("b", true)])).toBe(2);
  });

  it("never reports a failure while busy or loading", () => {
    const items = [user("pending")];
    expect(run(items, { busy: true, loading: false })).toBe(-1);
    expect(run(items, { busy: false, loading: true })).toBe(-1);
  });

  it("ignores an error that predates a successful later turn", () => {
    expect(run([user("a"), errorNotice(), user("b"), agent("recovered"), turnEnd()])).toBe(-1);
  });

  it("skips git-action chips when locating the last real user turn", () => {
    expect(run([user("real prompt"), user(`${APP_ACTION_PREFIX}git push`), errorNotice()])).toBe(0);
  });

  it("returns -1 for an empty log", () => {
    expect(run([])).toBe(-1);
  });
});
