// Every provider's tools land on a presenter, or on the name-and-input
// inference, so a run of steps summarizes the same way whoever ran it. The
// names here are the ones each adapter's reducer hands over (see
// tests/adapters/*): Claude's own, Codex's `shell`/`apply_patch`/
// `update_plan`/`exec_command`, Cursor's lowercase file tools, OpenCode's and
// Pi's lowercase names, Antigravity's `list_dir`.

import { describe, expect, it } from "vitest";
import { bashPresenter } from "@/components/Workspace/messages/presenters/Bash";
import { collabPresenter } from "@/components/Workspace/messages/presenters/Collab";
import {
  getPresenter,
  PRESENTERS,
  stepActivity,
  stepFacts,
} from "@/components/Workspace/messages/presenters/index";
import { familyOf } from "@/components/Workspace/messages/presenters/infer";
import { listPresenter } from "@/components/Workspace/messages/presenters/List";
import { parsePatch, patchPresenter } from "@/components/Workspace/messages/presenters/Patch";
import type { ToolCall } from "@/components/Workspace/messages/presenters/types";
import { summarizeSteps, summaryParts } from "@/components/Workspace/messages/steps";

const call = (name: string, input: unknown = {}): ToolCall =>
  ({ kind: "tool_call", id: "x", name, input }) as ToolCall;

describe("getPresenter across providers", () => {
  const table: [string, string, unknown][] = [
    // Codex
    ["codex", "shell", bashPresenter],
    ["codex", "exec_command", bashPresenter],
    ["codex", "exec", bashPresenter],
    ["codex", "apply_patch", patchPresenter],
    ["codex", "update_plan", PRESENTERS.TodoWrite],
    ["codex", "collab.wait", collabPresenter],
    // Cursor
    ["cursor", "shell", bashPresenter],
    ["cursor", "read", PRESENTERS.Read],
    ["cursor", "glob", PRESENTERS.Glob],
    ["cursor", "grep", PRESENTERS.Grep],
    ["cursor", "edit", PRESENTERS.Edit],
    ["cursor", "Task", PRESENTERS.Agent],
    // OpenCode
    ["opencode", "bash", bashPresenter],
    ["opencode", "read", PRESENTERS.Read],
    ["opencode", "write", PRESENTERS.Write],
    ["opencode", "edit", PRESENTERS.Edit],
    ["opencode", "grep", PRESENTERS.Grep],
    ["opencode", "glob", PRESENTERS.Glob],
    ["opencode", "list", listPresenter],
    ["opencode", "patch", patchPresenter],
    ["opencode", "webfetch", PRESENTERS.WebFetch],
    ["opencode", "todowrite", PRESENTERS.TodoWrite],
    // Pi
    ["pi", "bash", bashPresenter],
    ["pi", "read", PRESENTERS.Read],
    ["pi", "write", PRESENTERS.Write],
    ["pi", "edit", PRESENTERS.Edit],
    // Antigravity, Claude
    ["antigravity", "list_dir", listPresenter],
    ["claude", "LS", listPresenter],
  ];
  it.each(table)("%s's %s has a presenter", (_provider, name, presenter) => {
    expect(getPresenter(name)).toBe(presenter);
  });
});

describe("inference for tools without a presenter", () => {
  it("places a tool by the words of its name", () => {
    expect(familyOf("write_to_file")).toBe("change");
    expect(familyOf("replace_file_content")).toBe("change");
    expect(familyOf("search_replace")).toBe("change");
    expect(familyOf("run_command")).toBe("run");
    expect(familyOf("runTerminalCmd")).toBe("run");
    expect(familyOf("grep_search")).toBe("search");
    expect(familyOf("find_by_name")).toBe("search");
    expect(familyOf("codebase_search")).toBe("search");
    expect(familyOf("view_file")).toBe("read");
    expect(familyOf("read_url_content")).toBe("read");
    expect(familyOf("todo_write")).toBe("plan");
    expect(familyOf("mcp__fs__write-file")).toBe("change");
    // A name that doesn't say what it does stays unplaced.
    expect(familyOf("get_user")).toBe("other");
    expect(familyOf("Skill")).toBe("other");
  });

  it("reads the file, command or query off whatever key the provider uses", () => {
    expect(
      stepFacts(call("write_to_file", { TargetFile: "/p/a.ts", CodeContent: "x" }), null),
    ).toEqual({ kind: "change", path: "/p/a.ts" });
    expect(stepFacts(call("run_command", { CommandLine: "cd /p && npm test" }), null)).toEqual({
      kind: "run",
      subject: "npm test",
    });
    expect(stepFacts(call("grep_search", { Query: "useAuth", SearchPath: "/p" }), null)).toEqual({
      kind: "search",
      subject: "useAuth",
    });
    // Antigravity stores argument values JSON-encoded.
    expect(stepFacts(call("view_file", { AbsolutePath: '"/p/b.ts"' }), null)).toEqual({
      kind: "read",
      path: "/p/b.ts",
    });
    // A bare string is the command of a run tool.
    expect(stepFacts(call("exec_script", "python x.py"), null)).toEqual({
      kind: "run",
      subject: "python x.py",
    });
  });

  it("falls back to a named other step when a change or read names no file", () => {
    expect(stepFacts(call("delete_everything", {}), null)).toEqual({
      kind: "other",
      subject: "delete everything",
    });
    expect(stepFacts(call("get_user", { id: 1 }), null)).toEqual({
      kind: "other",
      subject: "get user",
    });
  });

  it("says what is happening on the live line", () => {
    expect(stepActivity(call("write_to_file", { TargetFile: "/p/a.ts" }))).toBe("Editing a.ts");
    expect(stepActivity(call("view_file", { AbsolutePath: "/p/b.ts" }))).toBe("Reading b.ts");
    expect(stepActivity(call("run_command", { CommandLine: "npm test" }))).toBe("Running npm test");
    expect(stepActivity(call("grep_search", { Query: "useAuth" }))).toBe("Searching useAuth");
    expect(stepActivity(call("todo_write", {}))).toBe("Updating the plan");
    expect(stepActivity(call("get_user", {}))).toBe("Using get user");
  });

  it("summarizes a run of unknown tools by what they did", () => {
    const items = [
      {
        kind: "tool_pair" as const,
        call: call("write_to_file", { TargetFile: "/p/a.ts" }),
        result: null,
      },
      {
        kind: "tool_pair" as const,
        call: call("run_command", { CommandLine: "npm test" }),
        result: null,
      },
      {
        kind: "tool_pair" as const,
        call: call("view_file", { AbsolutePath: "/p/b.ts" }),
        result: null,
      },
    ];
    expect(summaryParts(summarizeSteps(items)).map((p) => p.text)).toEqual([
      "changed a.ts",
      "ran",
      "read b.ts",
    ]);
  });
});

describe("patchPresenter", () => {
  const codexPatch = [
    "*** Begin Patch",
    "*** Update File: src/a.ts",
    "@@",
    " const a = 1;",
    "-const b = 2;",
    "+const b = 3;",
    "+const c = 4;",
    "*** Add File: src/new.ts",
    "+export const n = 1;",
    "*** Delete File: src/old.ts",
    "*** End Patch",
  ].join("\n");

  it("parses Codex's patch format into files and diff lines", () => {
    const files = parsePatch(codexPatch);
    expect(files.map((f) => [f.path, f.op, f.lines.length])).toEqual([
      ["src/a.ts", "update", 4],
      ["src/new.ts", "add", 1],
      ["src/old.ts", "delete", 0],
    ]);
    expect(files[0].lines.map((l) => l.op)).toEqual(["keep", "del", "add", "add"]);
  });

  it("reports a moved file where it ends up and remembers where it was", () => {
    const patch = [
      "*** Begin Patch",
      "*** Update File: src/old.ts",
      "*** Move to: src/new.ts",
      "@@",
      "-a",
      "+b",
      "*** End Patch",
    ].join("\n");
    expect(parsePatch(patch)).toEqual([
      {
        path: "src/new.ts",
        from: "src/old.ts",
        op: "update",
        lines: [
          { op: "del", text: "a" },
          { op: "add", text: "b" },
        ],
      },
    ]);
    expect(patchPresenter.facts?.(call("apply_patch", { patch }), null)?.paths).toEqual([
      "src/new.ts",
    ]);
    expect(patchPresenter.activity?.(call("apply_patch", patch))).toBe("Editing new.ts");
  });

  it("parses a unified diff too", () => {
    const files = parsePatch(
      [
        "--- a/x.ts",
        "+++ b/x.ts",
        "@@ -1 +1 @@",
        "-old",
        "+new",
        "--- /dev/null",
        "+++ b/y.ts",
        "+hi",
      ].join("\n"),
    );
    expect(files.map((f) => [f.path, f.op])).toEqual([
      ["x.ts", "update"],
      ["y.ts", "add"],
    ]);
  });

  it("counts every file the patch touched toward the run's summary", () => {
    const facts = patchPresenter.facts?.(call("apply_patch", { patch: codexPatch }), null);
    expect(facts).toEqual({
      kind: "change",
      paths: ["src/a.ts", "src/new.ts", "src/old.ts"],
      additions: 3,
      deletions: 1,
    });
    const s = summarizeSteps([
      { kind: "tool_pair", call: call("apply_patch", { patch: codexPatch }), result: null },
    ]);
    expect(s.changed).toEqual({
      paths: ["src/a.ts", "src/new.ts", "src/old.ts"],
      additions: 3,
      deletions: 1,
    });
    expect(summaryParts(s)[0].text).toBe("changed 3 files");
  });

  it("finds the patch text as a bare string or under a field", () => {
    expect(patchPresenter.activity?.(call("apply_patch", codexPatch))).toBe("Editing 3 files");
    expect(patchPresenter.activity?.(call("patch", { patchText: "*** Update File: a\n+x" }))).toBe(
      "Editing a",
    );
    expect(patchPresenter.activity?.(call("apply_patch", {}))).toBe("Applying a patch");
  });
});

describe("listPresenter", () => {
  it("names the directory under any key and counts it as a read", () => {
    expect(listPresenter.facts?.(call("list_dir", { DirectoryPath: '"/p/src"' }), null)).toEqual({
      kind: "read",
      path: "/p/src",
    });
    expect(listPresenter.activity?.(call("ls", { path: "/p/src/" }))).toBe("Listing src");
    expect(listPresenter.facts?.(call("list", {}), null)).toEqual({
      kind: "other",
      subject: "list",
    });
  });
});
