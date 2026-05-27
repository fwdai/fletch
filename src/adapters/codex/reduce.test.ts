import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { codexAdapter } from "./index";
import type { ChatItem, RawEvent } from "../types";

// NOTE: these tests assert against the *skeleton* adapter's current
// best-effort parsing. They will need to be updated when the real codex
// event shapes are verified — see TODO(codex-real-impl) in ./reduce.ts.

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  return readFileSync(join(here, "fixtures", name), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

describe("codexAdapter — skeleton", () => {
  it("reduces the sample fixture to a sensible ChatItem list", () => {
    const events = readJsonl("sample.jsonl") as RawEvent[];
    const items = events.reduce<ChatItem[]>(
      (acc, ev) => codexAdapter.reduce(acc, ev),
      [],
    );
    expect(items).toEqual([
      { kind: "user_message", text: "add a button" },
      { kind: "agent_message", text: "Adding it now.", streaming: false },
      {
        kind: "tool_call",
        id: "fc_1",
        name: "edit",
        input: { file: "App.tsx" },
        streaming: false,
      },
      {
        kind: "tool_result",
        tool_use_id: "fc_1",
        content: "ok",
        is_error: false,
      },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("returns prevItems unchanged for unknown event types", () => {
    const prev: ChatItem[] = [{ kind: "user_message", text: "hi" }];
    const next = codexAdapter.reduce(prev, { type: "??" } as RawEvent);
    expect(next).toBe(prev);
  });

  it("exposes id and policy on the adapter", () => {
    expect(codexAdapter.id).toBe("codex");
    expect(codexAdapter.policy["notice:turn_end"]).toBe("hide");
  });
});
