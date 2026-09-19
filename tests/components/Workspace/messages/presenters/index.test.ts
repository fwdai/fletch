import { describe, expect, it } from "vitest";
import { bashPresenter } from "@/components/Workspace/messages/presenters/Bash";
import { codegraphPresenter } from "@/components/Workspace/messages/presenters/Codegraph";
import { collabPresenter } from "@/components/Workspace/messages/presenters/Collab";
import { defaultPresenter } from "@/components/Workspace/messages/presenters/default";
import { getPresenter, PRESENTERS } from "@/components/Workspace/messages/presenters/index";
import type { ToolCall } from "@/components/Workspace/messages/presenters/types";

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

  it("renders a sub-agent Task (cursor, legacy Claude) with the Agent presenter", () => {
    expect(getPresenter("Task")).toBe(PRESENTERS.Agent);
  });

  it("routes codegraph tools across every adapter naming convention", () => {
    // Claude: raw mcp__<server>__<tool> passed through (case-insensitive).
    expect(getPresenter("mcp__codegraph__codegraph_explore")).toBe(codegraphPresenter);
    expect(getPresenter("MCP__CODEGRAPH__codegraph_explore")).toBe(codegraphPresenter);
    // Codex: <server>.<tool>, mcp__ stripped, dot-joined.
    expect(getPresenter("codegraph.codegraph_explore")).toBe(codegraphPresenter);
    // Underscore-joined server (e.g. opencode-style).
    expect(getPresenter("codegraph_codegraph_explore")).toBe(codegraphPresenter);
    // Any codegraph operation, not just explore.
    expect(getPresenter("codegraph.codegraph_search")).toBe(codegraphPresenter);
  });

  it("does not match other mcp servers", () => {
    expect(getPresenter("mcp__other__do_thing")).toBe(defaultPresenter);
    expect(getPresenter("other.do_thing")).toBe(defaultPresenter);
    // A separator must follow the `codegraph` token — no false positive on a
    // longer word that merely starts with it.
    expect(getPresenter("codegraphite_tool")).toBe(defaultPresenter);
  });

  it("falls back to the default presenter for unknown tools", () => {
    expect(getPresenter("someNovelTool")).toBe(defaultPresenter);
  });

  it("routes codex's collab.* coordination calls to the collab presenter", () => {
    expect(getPresenter("collab.wait")).toBe(collabPresenter);
    expect(getPresenter("collab.send_input")).toBe(collabPresenter);
    expect(getPresenter("collaborate")).toBe(defaultPresenter);
  });
});

describe("collabPresenter.summary", () => {
  const collab = (tool: string): ToolCall =>
    ({ kind: "tool_call", id: "x", name: `collab.${tool}`, input: {} }) as ToolCall;

  it("says what the parent is doing about its sub-agents", () => {
    expect(collabPresenter.summary(collab("wait"), null)).toBe("Waiting for sub-agents");
    expect(collabPresenter.summary(collab("send_input"), null)).toBe("Sending to sub-agent");
    expect(collabPresenter.summary(collab("close"), null)).toBe("Closing sub-agent");
  });

  it("degrades to the tool's own name for one it doesn't know", () => {
    expect(collabPresenter.summary(collab("resume_agent"), null)).toBe("Sub-agent resume agent");
  });

  it("titles the row Sub-agents instead of the raw collab.* name", () => {
    expect(collabPresenter.title).toBe("Sub-agents");
  });
});

describe("defaultPresenter.summary", () => {
  // Unregistered tools used to render `JSON.stringify(input)`, spending the
  // row on braces and quotes — `ToolSearch` showed up as
  // `{"max_results":3,"query":"select:mcp__codegraph__codegraph_explore"}`.
  it("renders a multi-field input as key/value pairs, not JSON", () => {
    expect(
      defaultPresenter.summary(
        toolCall({ max_results: 3, query: "select:mcp__codegraph__codegraph_explore" }),
        null,
      ),
    ).toBe("max_results 3 · query select:mcp__codegraph__codegraph_explore");
  });

  it("shows just the value when there is only one field", () => {
    expect(defaultPresenter.summary(toolCall({ query: "how does auth work" }), null)).toBe(
      "how does auth work",
    );
  });

  it("keeps a multi-line value on one line", () => {
    expect(defaultPresenter.summary(toolCall({ prompt: "line one\nline two" }), null)).toBe(
      "line one line two",
    );
  });

  it("skips empty fields", () => {
    expect(defaultPresenter.summary(toolCall({ query: "x", cursor: "", limit: null }), null)).toBe(
      "x",
    );
  });

  it("flattens nested values", () => {
    expect(
      defaultPresenter.summary(toolCall({ filter: { kind: "file" }, tags: ["a", "b"] }), null),
    ).toBe("filter kind file · tags a, b");
  });

  it("skips empty fields nested inside an object", () => {
    expect(
      defaultPresenter.summary(
        toolCall({ query: "x", filter: { kind: null, path: "", depth: 2 } }),
        null,
      ),
    ).toBe("query x · filter depth 2");
  });

  it("drops a field whose value is empty all the way down", () => {
    // Nothing left to say about `filter`, so it should not contribute a key.
    expect(
      defaultPresenter.summary(toolCall({ query: "x", filter: { kind: null, path: "" } }), null),
    ).toBe("x");
    expect(defaultPresenter.summary(toolCall({ filter: { kind: null } }), null)).toBe("");
    expect(defaultPresenter.summary(toolCall({ query: "x", tags: [], meta: {} }), null)).toBe("x");
  });

  it("keeps falsy values the tool was actually called with", () => {
    expect(defaultPresenter.summary(toolCall({ recursive: false, depth: 0 }), null)).toBe(
      "recursive false · depth 0",
    );
  });

  it("renders a bare string or array input", () => {
    expect(defaultPresenter.summary(toolCall("just a string"), null)).toBe("just a string");
    expect(defaultPresenter.summary(toolCall(["a", "b"]), null)).toBe("a, b");
  });

  it("renders nothing for an empty input", () => {
    expect(defaultPresenter.summary(toolCall({}), null)).toBe("");
    expect(defaultPresenter.summary(toolCall(null), null)).toBe("");
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
