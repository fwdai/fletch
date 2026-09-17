// The pending-hide filter every applied workspace snapshot passes through. The
// bug it pins: a snapshot fetched BEFORE the backend committed an archive (but
// applied after the user clicked) still lists the agent as live, and used to
// put the just-archived row back into the sidebar.

import { describe, expect, it } from "vitest";

import {
  applyPendingHides,
  clearPendingHide,
  markPendingHide,
  PENDING_ARCHIVE,
} from "./pendingHides";

// biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixtures
const ws = (agents: { id: string; archive?: unknown }[]) => ({ agents }) as any;

describe("applyPendingHides", () => {
  it("passes a snapshot through untouched when nothing is pending", () => {
    const snap = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(snap)).toBe(snap);
  });

  it("re-stamps a pending archive onto a snapshot that still shows the agent live", () => {
    markPendingHide("a", "archive");
    const out = applyPendingHides(
      ws([
        { id: "a", archive: null },
        { id: "b", archive: null },
      ]),
    );
    expect(out.agents.find((x) => x.id === "a")?.archive).toBe(PENDING_ARCHIVE);
    expect(out.agents.find((x) => x.id === "b")?.archive).toBeNull();
    clearPendingHide("a");
  });

  it("drops a pending discard from a snapshot that still lists the agent", () => {
    markPendingHide("a", "discard");
    const out = applyPendingHides(
      ws([
        { id: "a", archive: null },
        { id: "b", archive: null },
      ]),
    );
    expect(out.agents.map((x) => x.id)).toEqual(["b"]);
    clearPendingHide("a");
  });

  it("confirms an archive once a snapshot shows it and stops re-stamping", () => {
    markPendingHide("a", "archive");
    const real = { archived_at: "t", repos: [], diff_stats: { additions: 1, deletions: 0 } };
    const confirmed = applyPendingHides(ws([{ id: "a", archive: real }]));
    // Authoritative metadata wins over the placeholder.
    expect(confirmed.agents[0].archive).toBe(real);
    // The entry is gone: a later restore's un-archived snapshot is left alone.
    const restored = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(restored)).toBe(restored);
  });

  it("confirms a discard once the agent is absent", () => {
    markPendingHide("a", "discard");
    applyPendingHides(ws([{ id: "b", archive: null }]));
    const later = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(later)).toBe(later);
  });

  it("stops hiding once the hide is withdrawn", () => {
    markPendingHide("a", "archive");
    clearPendingHide("a");
    const snap = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(snap)).toBe(snap);
  });
});
