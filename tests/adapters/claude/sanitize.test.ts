import { describe, expect, it } from "vitest";
import { sanitizeUserText } from "@/adapters/claude/sanitize";

describe("sanitizeUserText", () => {
  it("returns plain text untouched", () => {
    const out = sanitizeUserText("hello world");
    expect(out).toEqual({ text: "hello world", notices: [] });
  });

  it("extracts a slash-command invocation", () => {
    const raw = [
      "<command-name>/login</command-name>",
      "<command-message>login</command-message>",
      "<command-args></command-args>",
      "<local-command-stdout>Login successful</local-command-stdout>",
    ].join("\n");
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("");
    expect(out.notices).toEqual([{ kind: "notice", subtype: "slash_command", text: "/login" }]);
  });

  it("ensures the slash prefix when the name lacks one", () => {
    const raw = "<command-name>clear</command-name><command-message>clear</command-message>";
    const out = sanitizeUserText(raw);
    expect(out.notices).toEqual([{ kind: "notice", subtype: "slash_command", text: "/clear" }]);
  });

  it("extracts a system-reminder as a hook_output notice", () => {
    const raw = "<system-reminder>Hook stderr: warn</system-reminder>";
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("");
    expect(out.notices).toEqual([
      { kind: "notice", subtype: "hook_output", text: "Hook stderr: warn" },
    ]);
  });

  it("preserves surrounding user text when wrappers are mixed in", () => {
    const raw = "Please run this:\n<system-reminder>session-reminder</system-reminder>\nThanks!";
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("Please run this:\n\nThanks!");
    expect(out.notices).toHaveLength(1);
    expect(out.notices[0].subtype).toBe("hook_output");
  });

  it("handles multiple wrappers in one message", () => {
    const raw = [
      "<command-name>/login</command-name><command-message>login</command-message>",
      "<system-reminder>ctx1</system-reminder>",
      "<system-reminder>ctx2</system-reminder>",
    ].join("\n");
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("");
    expect(out.notices.map((n) => n.subtype)).toEqual([
      "slash_command",
      "hook_output",
      "hook_output",
    ]);
  });

  it("ignores unknown wrapper tags", () => {
    const raw = "<unknown-tag>data</unknown-tag>";
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("<unknown-tag>data</unknown-tag>");
    expect(out.notices).toEqual([]);
  });

  it("converts a post-compact continuation preamble into a compact_summary notice", () => {
    const raw =
      "This session is being continued from a previous conversation that ran out of context.\n\nSummary: lorem ipsum…";
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("");
    expect(out.notices).toEqual([
      { kind: "notice", subtype: "compact_summary", text: "Conversation compacted" },
    ]);
  });

  it("drops empty system-reminders without emitting notices", () => {
    const raw = "<system-reminder>   </system-reminder>";
    const out = sanitizeUserText(raw);
    expect(out.text).toBe("");
    expect(out.notices).toEqual([]);
  });

  it("unwraps Cursor's timestamp/user_query envelope", () => {
    const raw = "<timestamp>Tue, Jun 9, 2026</timestamp>\n<user_query>\nHey\n</user_query>";
    expect(sanitizeUserText(raw).text).toBe("Hey");
  });

  it("strips an injected quorum-system block, even nested in the envelope", () => {
    const raw =
      "<timestamp>Tue, Jun 9, 2026</timestamp>\n<user_query>\n<quorum-system>\nfollow the rules\n</quorum-system>\n\nHey\n</user_query>";
    // Cleaned to exactly what the user typed, so it dedups against the
    // optimistic turn and renders as a single clean bubble.
    expect(sanitizeUserText(raw).text).toBe("Hey");
  });
});
