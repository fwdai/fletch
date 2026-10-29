import { describe, expect, it } from "vitest";
import type { BoxEgressEvent } from "@/api";
import { foldBlocked } from "./sandbox";

const denied = (agent_id: string, host: string, quiet = false): BoxEgressEvent => ({
  agent_id,
  host,
  port: 443,
  allowed: false,
  quiet,
});

describe("foldBlocked", () => {
  it("counts repeat connections on one entry per workspace and host", () => {
    let blocked = foldBlocked([], denied("a", "x.com"));
    blocked = foldBlocked(blocked, denied("a", "x.com"));
    blocked = foldBlocked(blocked, denied("b", "x.com"));
    expect(blocked.map((b) => [b.agent_id, b.host, b.count])).toEqual([
      ["a", "x.com", 2],
      ["b", "x.com", 1],
    ]);
  });

  it("leaves a new host unseen, and noise seen from the start", () => {
    const blocked = foldBlocked(
      foldBlocked([], denied("a", "x.com")),
      denied("a", "o1.ingest.sentry.io", true),
    );
    expect(blocked.map((b) => [b.host, b.seen])).toEqual([
      ["x.com", false],
      ["o1.ingest.sentry.io", true],
    ]);
  });

  it("does not prompt again for a seen host when it is refused again", () => {
    const first = foldBlocked([], denied("a", "x.com")).map((b) => ({ ...b, seen: true }));
    expect(foldBlocked(first, denied("a", "x.com"))[0].seen).toBe(true);
  });

  it("keeps a bounded list", () => {
    let blocked = foldBlocked([], denied("a", "h0.com"));
    for (let i = 1; i < 250; i++) blocked = foldBlocked(blocked, denied("a", `h${i}.com`));
    expect(blocked).toHaveLength(200);
    expect(blocked[0].host).toBe("h50.com");
  });
});
