import { describe, expect, it } from "vitest";
import type { ViewItem } from "../messages/pair";
import { endedOnAccountError } from "./accountError";

const user = (text: string): ViewItem => ({ kind: "user_message", text });
const reply = (text: string): ViewItem => ({ kind: "agent_message", text });
const error = (text: string): ViewItem => ({
  kind: "notice",
  subtype: "error",
  text,
  is_error: true,
});
const turnEnd = (text: string): ViewItem => ({ kind: "notice", subtype: "turn_end", text });

describe("endedOnAccountError", () => {
  it("flags a turn whose error notice reports a usage limit", () => {
    const items = [
      user("go"),
      error("You've hit your usage limit. Try again at 5pm."),
      turnEnd("error"),
    ];
    expect(endedOnAccountError(items)).toBe(true);
  });

  it("flags a turn whose error notice asks the user to log in", () => {
    const items = [user("go"), error("Invalid API key · Please run /login"), turnEnd("error")];
    expect(endedOnAccountError(items)).toBe(true);
  });

  it("flags a failed turn whose limit message is in the agent's reply", () => {
    const items = [
      user("go"),
      reply("Claude AI usage limit reached|1760000000"),
      error("Turn failed"),
      turnEnd("error"),
    ];
    expect(endedOnAccountError(items)).toBe(true);
  });

  it("ignores a failed turn with an unrelated error", () => {
    const items = [user("go"), error("Process exited with code 1"), turnEnd("error")];
    expect(endedOnAccountError(items)).toBe(false);
  });

  it("ignores a successful turn that merely mentions a rate limit", () => {
    const items = [
      user("what is a rate limit?"),
      reply("A rate limit caps requests."),
      turnEnd("success"),
    ];
    expect(endedOnAccountError(items)).toBe(false);
  });

  it("ignores a limit error from a turn before the latest user message", () => {
    const items = [user("go"), error("usage limit reached"), turnEnd("error"), user("again")];
    expect(endedOnAccountError(items)).toBe(false);
  });
});
