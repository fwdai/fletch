import { describe, expect, it } from "vitest";
import type { SlashCommand } from "@/data/slashCommands";
import {
  EXPANSION_SEPARATOR,
  expandCommandText,
  expandedCommandLine,
  substitutePromptArgs,
} from "@/helpers";

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

  it("expands a bodied command: typed invocation, separator, substituted body", () => {
    expect(expandCommandText(commands, "/draftpr fix login")).toBe(
      `/draftpr fix login${EXPANSION_SEPARATOR}Open a PR: fix login`,
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
      `/draftpr fix\nlogin${EXPANSION_SEPARATOR}Open a PR: fix login`,
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

  it("folds an expansion round-trip back to its typed invocation", () => {
    const sent = expandCommandText(commands, "/draftpr fix login");
    expect(sent).not.toBeNull();
    expect(expandedCommandLine(commands, sent as string)).toBe("/draftpr fix login");
  });

  it("folds a multiline typed invocation whole", () => {
    const sent = expandCommandText(commands, "/draftpr fix\nlogin");
    expect(expandedCommandLine(commands, sent as string)).toBe("/draftpr fix\nlogin");
  });

  it("never folds a literal message, even one byte-equal to an expansion sans separator", () => {
    // The regression pair: literal messages sent before the `draftpr` prompt
    // existed. Neither a same-name multiline message nor a message that
    // exactly reproduces the expansion text can fold — the zero-width-space
    // separator only ever comes from the app's own expansion.
    expect(
      expandedCommandLine(commands, "/draftpr fix login\nalso please update the changelog"),
    ).toBeNull();
    expect(expandedCommandLine(commands, "/draftpr fix login\n\nOpen a PR: fix login")).toBeNull();
  });

  it("ignores separator-bearing text whose name isn't a known bodied command", () => {
    expect(expandedCommandLine(commands, `/other x${EXPANSION_SEPARATOR}Open a PR: x`)).toBeNull();
  });

  it("ignores single-line and non-slash texts", () => {
    expect(expandedCommandLine(commands, "/draftpr fix login")).toBeNull();
    expect(expandedCommandLine(commands, "plain text\nwith lines")).toBeNull();
  });
});
