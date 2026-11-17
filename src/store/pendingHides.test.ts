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

const H = "local";

// biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixtures
const ws = (agents: { id: string; archive?: unknown }[]) => ({ agents }) as any;

describe("applyPendingHides", () => {
  it("passes a snapshot through untouched when nothing is pending", () => {
    const snap = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(H, snap)).toBe(snap);
  });

  it("re-stamps a pending archive onto a snapshot that still shows the agent live", () => {
    markPendingHide(H, "a", "archive");
    const out = applyPendingHides(
      H,
      ws([
        { id: "a", archive: null },
        { id: "b", archive: null },
      ]),
    );
    expect(out.agents.find((x) => x.id === "a")?.archive).toBe(PENDING_ARCHIVE);
    expect(out.agents.find((x) => x.id === "b")?.archive).toBeNull();
    clearPendingHide(H, "a");
  });

  it("drops a pending discard from a snapshot that still lists the agent", () => {
    markPendingHide(H, "a", "discard");
    const out = applyPendingHides(
      H,
      ws([
        { id: "a", archive: null },
        { id: "b", archive: null },
      ]),
    );
    expect(out.agents.map((x) => x.id)).toEqual(["b"]);
    clearPendingHide(H, "a");
  });

  it("confirms an archive once a snapshot shows it and stops re-stamping", () => {
    markPendingHide(H, "a", "archive");
    const real = { archived_at: "t", repos: [], diff_stats: { additions: 1, deletions: 0 } };
    const confirmed = applyPendingHides(H, ws([{ id: "a", archive: real }]));
    // Authoritative metadata wins over the placeholder.
    expect(confirmed.agents[0].archive).toBe(real);
    // The entry is gone: a later restore's un-archived snapshot is left alone.
    const restored = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(H, restored)).toBe(restored);
  });

  it("confirms a discard once the agent is absent", () => {
    markPendingHide(H, "a", "discard");
    applyPendingHides(H, ws([{ id: "b", archive: null }]));
    const later = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(H, later)).toBe(later);
  });

  it("hides a row only in the snapshots of the host it was asked of", () => {
    // Two hosts both have an agent called `a`: archiving one must not hide
    // the other's.
    markPendingHide(H, "a", "archive");
    const other = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides("host-2", other)).toBe(other);
    clearPendingHide(H, "a");
  });

  it("stops hiding once the hide is withdrawn", () => {
    markPendingHide(H, "a", "archive");
    clearPendingHide(H, "a");
    const snap = ws([{ id: "a", archive: null }]);
    expect(applyPendingHides(H, snap)).toBe(snap);
  });
});
