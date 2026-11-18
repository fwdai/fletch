import { describe, expect, it } from "vitest";
import { followTurn, reconcileTurns } from "./turn";

describe("followTurn", () => {
  it("takes the host's word for an open turn, whatever the status", () => {
    // A resting session's message: the turn is open while its process comes
    // up, rests, and only then runs.
    let turns = followTurn({}, "fuji", "idle", 100);
    expect(turns).toEqual({ fuji: 100 });
    turns = followTurn(turns, "fuji", "spawning", 100);
    turns = followTurn(turns, "fuji", "idle", 100);
    turns = followTurn(turns, "fuji", "running", 100);
    expect(turns).toEqual({ fuji: 100 });
  });

  it("closes the turn when the host says there is none", () => {
    expect(followTurn({ fuji: 100, etna: 7 }, "fuji", "idle", null)).toEqual({ etna: 7 });
  });

  it("returns the same object when nothing changes", () => {
    const turns = { fuji: 100 };
    expect(followTurn(turns, "fuji", "running", 100)).toBe(turns);
    expect(followTurn(turns, "etna", "idle", null)).toBe(turns);
  });

  describe("from a host that does not say", () => {
    it("keeps the turn while the agent is running or spawning", () => {
      const turns = { fuji: 100 };
      expect(followTurn(turns, "fuji", "running", undefined)).toBe(turns);
      expect(followTurn(turns, "fuji", "spawning", undefined)).toBe(turns);
    });

    it("ends it at any other status", () => {
      for (const status of ["idle", "error", "stopped"] as const) {
        expect(followTurn({ fuji: 100 }, "fuji", status, undefined)).toEqual({});
      }
    });
  });
});

describe("reconcileTurns", () => {
  it("takes each record's word, open or closed", () => {
    const turns = reconcileTurns({ fuji: 100, etna: 7 }, [
      { id: "fuji", turn_since: null },
      { id: "etna", turn_since: 9 },
      { id: "kibo", turn_since: 3 },
    ]);
    expect(turns).toEqual({ etna: 9, kibo: 3 });
  });

  it("leaves an agent whose record does not say, and one not in the snapshot", () => {
    const turns = { fuji: 100, etna: 7 };
    expect(reconcileTurns(turns, [{ id: "fuji" }])).toBe(turns);
  });

  it("returns the same object when the snapshot agrees", () => {
    const turns = { fuji: 100 };
    expect(reconcileTurns(turns, [{ id: "fuji", turn_since: 100 }])).toBe(turns);
  });
});
