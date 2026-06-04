import { describe, expect, it } from "vitest";

import { reduceStoredEvent } from "./store";
import type { ChatItem, RawEvent } from "./adapters";

/**
 * Restore replays every persisted `session_events` row through
 * `reduceStoredEvent`. Older histories can hold the same initial
 * `user_message` event repeated — a send retried while the agent was still
 * spawning used to persist one copy per attempt. Replay must collapse those
 * so the prompt renders once, not N times.
 */
describe("reduceStoredEvent — user_message dedup", () => {
  const userEvent = (text: string): RawEvent =>
    ({ type: "user_message", text, attachments: [] }) as unknown as RawEvent;

  it("collapses consecutive identical user_message events", () => {
    let items: ChatItem[] = [];
    for (let i = 0; i < 3; i += 1) {
      items = reduceStoredEvent("claude", items, userEvent("read the readme"));
    }
    expect(items).toEqual([{ kind: "user_message", text: "read the readme" }]);
  });

  it("keeps distinct user_message events", () => {
    let items: ChatItem[] = [];
    items = reduceStoredEvent("claude", items, userEvent("first"));
    items = reduceStoredEvent("claude", items, userEvent("second"));
    expect(items).toEqual([
      { kind: "user_message", text: "first" },
      { kind: "user_message", text: "second" },
    ]);
  });

  it("collapses consecutive identical passthrough slash-command notices", () => {
    let items: ChatItem[] = [];
    for (let i = 0; i < 3; i += 1) {
      items = reduceStoredEvent("claude", items, userEvent("/compact"));
    }
    expect(items).toEqual([
      { kind: "notice", subtype: "slash_command", text: "/compact" },
    ]);
  });
});
