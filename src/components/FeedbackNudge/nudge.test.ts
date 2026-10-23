import { describe, expect, it, vi } from "vitest";
import { createScheduler, isDue, localDay, type Nudge, parseNudge, recordDay } from "./nudge";

describe("feedback nudge schedule", () => {
  it("records the first day and is not due", () => {
    const n = recordDay({ days: [], shown: false }, "2026-10-01");
    expect(n).toEqual({ days: ["2026-10-01"], shown: false });
    expect(isDue(n)).toBe(false);
  });

  it("does not count a second open on the same day", () => {
    const prev = { days: ["2026-10-01"], shown: false };
    expect(recordDay(prev, "2026-10-01")).toBe(prev);
  });

  it("is due on the third distinct day, however far apart", () => {
    const d1 = recordDay({ days: [], shown: false }, "2026-10-01");
    const d2 = recordDay(d1, "2026-10-14");
    expect(isDue(d2)).toBe(false);
    expect(isDue(recordDay(d2, "2026-12-25"))).toBe(true);
  });

  it("is never due again once shown", () => {
    expect(isDue({ days: ["a", "b", "c", "d"], shown: true })).toBe(false);
  });

  it("tolerates a missing or broken row", () => {
    expect(parseNudge(null)).toEqual({ days: [], shown: false });
    expect(parseNudge("not json")).toEqual({ days: [], shown: false });
    expect(parseNudge('{"days":"x","shown":"yes"}')).toEqual({ days: [], shown: false });
  });

  it("round-trips through JSON", () => {
    const n = { days: ["2026-10-01", "2026-10-02"], shown: false };
    expect(parseNudge(JSON.stringify(n))).toEqual(n);
  });

  it("formats the local day with zero padding", () => {
    expect(localDay(new Date(2026, 0, 5))).toBe("2026-01-05");
  });
});

/** A scheduler over an in-memory settings row, with its collaborators spied. */
function harness(stored: Nudge | null, today = "2026-10-03") {
  let row = stored ? JSON.stringify(stored) : null;
  const saves: Nudge[] = [];
  const open = vi.fn();
  const load = vi.fn(async () => row);
  const tick = createScheduler({
    load,
    save: async (n) => {
      saves.push(n);
      row = JSON.stringify(n);
    },
    open,
    today: () => today,
  });
  return { tick, open, load, saves, stored: () => parseNudge(row) };
}

const TWO_DAYS: Nudge = { days: ["2026-10-01", "2026-10-02"], shown: false };

describe("feedback nudge scheduler", () => {
  it("does nothing before settings are hydrated", async () => {
    const h = harness(TWO_DAYS);
    await h.tick({ hydrated: false, onboardingOpen: false });
    expect(h.load).not.toHaveBeenCalled();
    expect(h.open).not.toHaveBeenCalled();
  });

  // The ordering bug: the store says onboarding is closed until hydration
  // says otherwise. A third-day start with the tour unfinished must record the
  // day, then wait — not raise the prompt under the tour and spend it.
  it("on the third day with onboarding still open: records the day, waits, then shows once it closes", async () => {
    const h = harness(TWO_DAYS);
    await h.tick({ hydrated: false, onboardingOpen: false });
    await h.tick({ hydrated: true, onboardingOpen: true });

    expect(h.stored().days).toEqual(["2026-10-01", "2026-10-02", "2026-10-03"]);
    expect(h.stored().shown).toBe(false);
    expect(h.open).not.toHaveBeenCalled();

    await h.tick({ hydrated: true, onboardingOpen: false });
    expect(h.open).toHaveBeenCalledTimes(1);
    expect(h.stored().shown).toBe(true);
  });

  it("marks shown before opening, so a quit mid-prompt still counts", async () => {
    const h = harness(TWO_DAYS);
    let shownAtOpen: boolean | null = null;
    const tick = createScheduler({
      load: async () => JSON.stringify(TWO_DAYS),
      save: async (n) => {
        shownAtOpen = n.shown;
      },
      open: () => {
        expect(shownAtOpen).toBe(true);
        h.open();
      },
      today: () => "2026-10-03",
    });
    await tick({ hydrated: true, onboardingOpen: false });
    expect(h.open).toHaveBeenCalledTimes(1);
  });

  it("shows once per session even when onboarding flips repeatedly", async () => {
    const h = harness(TWO_DAYS);
    await h.tick({ hydrated: true, onboardingOpen: false });
    await h.tick({ hydrated: true, onboardingOpen: true });
    await h.tick({ hydrated: true, onboardingOpen: false });
    expect(h.open).toHaveBeenCalledTimes(1);
    expect(h.saves.filter((s) => s.shown)).toHaveLength(1);
  });

  it("serializes ticks that race the DB read", async () => {
    const h = harness(TWO_DAYS);
    // Both ticks land before the first load resolves; the second must see the
    // first's result, not re-read and act on stale state.
    await Promise.all([
      h.tick({ hydrated: true, onboardingOpen: true }),
      h.tick({ hydrated: true, onboardingOpen: false }),
    ]);
    expect(h.load).toHaveBeenCalledTimes(1);
    expect(h.open).toHaveBeenCalledTimes(1);
  });

  it("on an early day records it and stays quiet, even with onboarding closed", async () => {
    const h = harness({ days: ["2026-10-01"], shown: false });
    await h.tick({ hydrated: true, onboardingOpen: false });
    expect(h.stored().days).toEqual(["2026-10-01", "2026-10-03"]);
    expect(h.open).not.toHaveBeenCalled();
  });

  it("never shows again once shown, and writes nothing", async () => {
    const h = harness(
      { days: ["2026-10-01", "2026-10-02", "2026-10-03"], shown: true },
      "2026-10-09",
    );
    await h.tick({ hydrated: true, onboardingOpen: false });
    expect(h.open).not.toHaveBeenCalled();
    // The day is still appended (one write) — harmless bookkeeping; shown stays.
    expect(h.stored().shown).toBe(true);
  });

  it("swallows a failing DB rather than surfacing", async () => {
    const tick = createScheduler({
      load: async () => {
        throw new Error("db down");
      },
      save: async () => {},
      open: vi.fn(),
      today: () => "2026-10-03",
    });
    await expect(tick({ hydrated: true, onboardingOpen: false })).resolves.toBeUndefined();
  });
});
