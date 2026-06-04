/**
 * Proves the backend durable-event filter is lossless.
 *
 * The Rust backend persists only "durable" provider events (dropping ephemeral
 * streaming deltas and no-op lifecycle events), then the frontend replays them
 * through each provider's reduce() on restore. This test verifies that for each
 * provider, reducing the DURABLE-only subset yields the IDENTICAL ChatItem[] as
 * reducing the FULL event stream — i.e. the filter loses nothing.
 */

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { getAdapter } from "./index";
import type { ChatItem, RawEvent } from "./types";

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(path: string): RawEvent[] {
  const raw = readFileSync(path, "utf8");
  return raw
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l) as RawEvent);
}

function reduceAll(provider: string, events: RawEvent[]): ChatItem[] {
  const adapter = getAdapter(provider);
  return events.reduce<ChatItem[]>((acc, ev) => adapter.reduce(acc, ev), []);
}

/**
 * Mirror of the Rust backend `is_durable_event` predicate.
 * Only durable events are persisted to SQLite for replay on restore.
 */
function isDurable(provider: string, ev: RawEvent): boolean {
  const t = ev.type;
  switch (provider) {
    case "claude":
      return t === "assistant" || t === "user" || t === "result";
    case "codex":
      return (
        t === "item.completed" ||
        t === "turn.completed" ||
        t === "turn.failed" ||
        t === "error"
      );
    case "opencode": {
      if (t === "text" || t === "step_finish" || t === "error") return true;
      if (t === "tool_use") {
        const status = (ev as Record<string, unknown> & { part?: { state?: { status?: string } } })
          .part?.state?.status;
        return status === "completed" || status === "error";
      }
      return false;
    }
    case "pi":
      return (
        t === "message_end" || t === "tool_execution_end" || t === "agent_end"
      );
    case "cursor": {
      if (t === "assistant" || t === "user" || t === "result") return true;
      if (t === "tool_call") return (ev as Record<string, unknown>).subtype === "completed";
      return false;
    }
    default:
      return true;
  }
}

interface ProviderCase {
  provider: string;
  fixture: string;
}

const cases: ProviderCase[] = [
  {
    provider: "claude",
    fixture: join(here, "claude/fixtures/live-events.jsonl"),
  },
  {
    provider: "codex",
    fixture: join(here, "codex/fixtures/sample.jsonl"),
  },
  {
    provider: "opencode",
    fixture: join(here, "opencode/fixtures/sample.jsonl"),
  },
  {
    provider: "pi",
    fixture: join(here, "pi/fixtures/sample.jsonl"),
  },
  {
    provider: "cursor",
    fixture: join(here, "cursor/fixtures/sample.jsonl"),
  },
];

describe("durable-event filter is lossless", () => {
  it.each(cases)("$provider: durable subset reproduces full render", ({ provider, fixture }) => {
    const events = readJsonl(fixture);
    const durableEvents = events.filter((ev) => isDurable(provider, ev));

    // Assert the fixture is meaningful: it contains at least some ephemeral events.
    // For the primary provider (claude) this is mandatory — its live stream has
    // stream_event deltas. For others, log a notice if all events happen to be
    // durable already (the lossless assertion still holds trivially).
    const hasEphemeral = events.length > durableEvents.length;
    if (provider === "claude") {
      expect(
        hasEphemeral,
        `${provider} fixture must contain ephemeral events (stream_event deltas) to be a meaningful test`,
      ).toBe(true);
    } else if (!hasEphemeral) {
      console.log(
        `[durable-lossless] ${provider}: fixture contains only durable events — lossless assertion is trivially true`,
      );
    }

    const full = reduceAll(provider, events);
    const durable = reduceAll(provider, durableEvents);

    // The core invariant: replaying only durable events must yield the identical
    // final ChatItem[] as replaying the full stream.
    expect(durable).toEqual(full);
  });
});
