import { describe, expect, it } from "vitest";
import { dischargeSending, reconcileSending } from "./sending";

describe("dischargeSending", () => {
  it("is answered by the running the send produced", () => {
    expect(dischargeSending({ fuji: true }, "fuji", "idle", "running")).toEqual({});
  });

  it("is answered by a failure to run, and by an idle the send did not account for", () => {
    expect(dischargeSending({ fuji: true }, "fuji", "idle", "error")).toEqual({});
    expect(dischargeSending({ fuji: true }, "fuji", "running", "stopped")).toEqual({});
    expect(dischargeSending({ fuji: true }, "fuji", "idle", "idle")).toEqual({});
    expect(dischargeSending({ fuji: true }, "fuji", undefined, "idle")).toEqual({});
  });

  it("survives the spawn a send to a dead agent triggers, and that spawn's resting idle", () => {
    const sending = { fuji: true };
    expect(dischargeSending(sending, "fuji", "idle", "spawning")).toBe(sending);
    expect(dischargeSending(sending, "fuji", "spawning", "idle")).toBe(sending);
    // …and goes once the held message becomes the first turn.
    expect(dischargeSending(sending, "fuji", "idle", "running")).toEqual({});
  });

  it("leaves other agents' flags and an absent flag untouched", () => {
    const sending = { denali: true };
    expect(dischargeSending(sending, "fuji", "idle", "running")).toBe(sending);
    expect(dischargeSending(sending, "denali", "idle", "running")).toEqual({});
  });
});

describe("reconcileSending", () => {
  const at = (id: string, status: "idle" | "running" | "spawning" | "stopped" | "error") => ({
    id,
    status,
  });

  it("settles a flag the snapshot shows at rest", () => {
    const next = reconcileSending({ fuji: true, denali: true }, [at("fuji", "idle")], new Set());
    expect(next).toEqual({ denali: true });
  });

  it("keeps a flag the snapshot shows busy, or whose send is still on the wire", () => {
    const sending = { fuji: true, denali: true };
    expect(
      reconcileSending(sending, [at("fuji", "running"), at("denali", "spawning")], new Set()),
    ).toBe(sending);
    expect(reconcileSending(sending, [at("fuji", "idle")], new Set(["fuji"]))).toBe(sending);
  });

  it("ignores agents the snapshot does not carry", () => {
    const sending = { fuji: true };
    expect(reconcileSending(sending, [at("denali", "idle")], new Set())).toBe(sending);
  });
});
