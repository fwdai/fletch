import { describe, expect, it } from "vitest";
import type { SlashCommand } from "@/data/slashCommands";
import { expandCommandText, expandedCommandLine, substitutePromptArgs } from "@/helpers";

describe("substitutePromptArgs", () => {
  it("substitutes positionals and $ARGUMENTS", () => {
    expect(substitutePromptArgs("fix $1 then $2", "auth.ts db.ts")).toBe("fix auth.ts then db.ts");
    expect(substitutePromptArgs("review: $ARGUMENTS", "a b c")).toBe("review: a b c");
  });

  it("resolves named placeholders from KEY=value args, honoring quotes", () => {
    expect(
      substitutePromptArgs(
        "open a PR for $FILE titled $TITLE",
        'FILE=src/a.ts TITLE="Add animation"',
      ),
    ).toBe("open a PR for src/a.ts titled Add animation");
  });

  it("keeps quoted positionals whole", () => {
    expect(substitutePromptArgs("check $1", '"a b.ts" extra')).toBe("check a b.ts");
  });

  it("emits a literal $ for $$ and leaves unmatched named placeholders alone", () => {
    expect(substitutePromptArgs("costs $$5 in $PATH for $1", "x")).toBe("costs $5 in $PATH for x");
  });

  it("replaces a missing positional with nothing", () => {
    expect(substitutePromptArgs("do $1 and $2", "only")).toBe("do only and ");
  });

  it("appends args when the body uses no placeholders, so they're never dropped", () => {
    expect(substitutePromptArgs("just do the thing", "src/a.ts")).toBe(
      "just do the thing\n\nsrc/a.ts",
    );
    expect(substitutePromptArgs("just do the thing", "")).toBe("just do the thing");
    // `$$` alone is an escape, not a placeholder — args still append.
    expect(substitutePromptArgs("pay $$10", "now")).toBe("pay $10\n\nnow");
  });
});

describe("expandCommandText", () => {
  const commands: SlashCommand[] = [
    {
      kind: "passthrough",
      name: "draftpr",
      description: "Draft a PR",
      body: "Open a PR: $ARGUMENTS",
    },
    { kind: "passthrough", name: "init", description: "CLI-resolved, no body" },
    { kind: "local", name: "clear", description: "", action: "app:clear" },
  ];

  it("expands a bodied command, keeping the typed invocation as the first line", () => {
    expect(expandCommandText(commands, "/draftpr fix login")).toBe(
      "/draftpr fix login\n\nOpen a PR: fix login",
    );
  });

  it("passes verbatim commands, local commands, and plain text through untouched", () => {
    expect(expandCommandText(commands, "/init")).toBeNull();
    expect(expandCommandText(commands, "/clear")).toBeNull();
    expect(expandCommandText(commands, "/unknown thing")).toBeNull();
    expect(expandCommandText(commands, "hello /draftpr")).toBeNull();
    expect(expandCommandText(commands, "/")).toBeNull();
  });

  it("carries multi-line arguments into the expansion", () => {
    expect(expandCommandText(commands, "/draftpr fix\nlogin")).toBe(
      "/draftpr fix\nlogin\n\nOpen a PR: fix login",
    );
  });
});

describe("expandedCommandLine", () => {
  const commands: SlashCommand[] = [
    {
      kind: "passthrough",
      name: "draftpr",
      description: "Draft a PR",
      body: "Open a PR: $ARGUMENTS",
    },
  ];

  it("folds a message that is exactly what expansion produces", () => {
    const sent = expandCommandText(commands, "/draftpr fix login");
    expect(sent).not.toBeNull();
    expect(expandedCommandLine(commands, sent as string)).toBe("/draftpr fix login");
  });

  it("never folds an ordinary message that merely starts with a bodied command's name", () => {
    // The regression: this literal message predates the `draftpr` prompt (or
    // was sent unexpanded for any reason). A later discovery of the name must
    // not hide its body — recomputation fails the comparison.
    expect(
      expandedCommandLine(commands, "/draftpr fix login\nalso please update the changelog"),
    ).toBeNull();
  });

  it("unfolds when the prompt body has been edited since the send", () => {
    const sentWithOldBody = "/draftpr fix login\n\nOld body: fix login";
    expect(expandedCommandLine(commands, sentWithOldBody)).toBeNull();
  });

  it("ignores single-line, unknown, and non-slash texts", () => {
    expect(expandedCommandLine(commands, "/draftpr fix login")).toBeNull();
    expect(expandedCommandLine(commands, "/other x\n\nOpen a PR: x")).toBeNull();
    expect(expandedCommandLine(commands, "plain text\nwith lines")).toBeNull();
  });
});
