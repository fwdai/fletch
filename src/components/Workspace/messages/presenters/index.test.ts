import { describe, expect, it } from "vitest";
import { bashPresenter } from "./Bash";
import { defaultPresenter } from "./default";
import { getPresenter, PRESENTERS } from "./index";
import type { ToolCall } from "./types";

const toolCall = (input: unknown): ToolCall =>
  ({ kind: "tool_call", id: "x", name: "shell", input }) as ToolCall;

describe("getPresenter", () => {
  it("matches canonical Claude names", () => {
    expect(getPresenter("Bash")).toBe(PRESENTERS.Bash);
    expect(getPresenter("Read")).toBe(PRESENTERS.Read);
  });

  it("matches case-insensitively (cursor's lowercase names)", () => {
    expect(getPresenter("read")).toBe(PRESENTERS.Read);
    expect(getPresenter("glob")).toBe(PRESENTERS.Glob);
    expect(getPresenter("GREP")).toBe(PRESENTERS.Grep);
  });

  it("resolves cross-provider renames (cursor shell → Bash)", () => {
    expect(getPresenter("shell")).toBe(PRESENTERS.Bash);
  });

  it("falls back to the default presenter for unknown tools", () => {
    expect(getPresenter("someNovelTool")).toBe(defaultPresenter);
  });
});

describe("bashPresenter.summary", () => {
  // Claude's `Bash` hands over an object input; Codex/Cursor `shell` hand over
  // the command as a bare value. All three must render the command, not the
  // "(no command)" fallback.
  it("renders Claude's { command } object shape", () => {
    expect(bashPresenter.summary(toolCall({ command: "ls -la" }), null)).toBe("ls -la");
  });

  it("renders Codex/Cursor's bare command string", () => {
    expect(bashPresenter.summary(toolCall("/bin/zsh -lc 'echo hi'"), null)).toBe(
      "/bin/zsh -lc 'echo hi'",
    );
  });

  it("renders a bare argv array", () => {
    expect(bashPresenter.summary(toolCall(["bash", "-lc", "echo hi"]), null)).toBe(
      "bash -lc echo hi",
    );
  });

  it("falls back to (no command) when there is genuinely no command", () => {
    expect(bashPresenter.summary(toolCall({}), null)).toBe("(no command)");
  });
});
