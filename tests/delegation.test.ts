// What is left of the delegation module on the client: the trigger format and
// the copy. The decisions (`delegationResolved`, `delegationStep`,
// `actionProvesKind`, …) moved to the host with their cases —
// crates/fletch-core/src/supervisor/delegation_tests.rs.

import { describe, expect, it } from "vitest";
import {
  APP_ACTION_PREFIX,
  appActionMessage,
  DELEGATION_KINDS,
  delegationLabel,
} from "@/delegation";

describe("appActionMessage", () => {
  // The shared fixture: the host's `app_action_message` is pinned against these
  // same strings (`the_trigger_matches_the_typescript_fixture`), so a client
  // falling back to a plain send on an older host sends exactly what the host
  // would have composed.
  it.each([
    ["commit", undefined, "[app-action] commit"],
    ["commit-pr", { base: "main" }, '[app-action] commit-pr base="main"'],
    [
      "fix-checks",
      { failing: "unit, lint", repo: "web" },
      '[app-action] fix-checks failing="unit, lint" repo="web"',
    ],
    ["open-pr", { base: "" }, "[app-action] open-pr"],
    ["fix-checks", { failing: 'say "hi"' }, '[app-action] fix-checks failing="say \\"hi\\""'],
  ])("%s %o", (name, params, expected) => {
    expect(appActionMessage(name, params)).toBe(expected);
  });

  it("triggers start with the shared prefix the transcript folds into a chip", () => {
    expect(appActionMessage("commit").startsWith(APP_ACTION_PREFIX)).toBe(true);
  });
});

describe("copy", () => {
  it("has a label for every kind", () => {
    // Iterating DELEGATION_KINDS, not a hand-written list — the hand-written one
    // had silently fallen two kinds behind while still claiming "every kind".
    for (const k of DELEGATION_KINDS) {
      expect(delegationLabel(k).length).toBeGreaterThan(0);
    }
  });
});
