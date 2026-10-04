import { describe, expect, it } from "vitest";
import type { PrState } from "@/api";
import { type FocusedPr, focusedPrReads, parsePrSignature, prSignature } from "./focusedPrReads";

const pr = (over: Partial<FocusedPr> = {}): FocusedPr => ({
  key: "fuji",
  number: 650,
  open: true,
  checks: true,
  threads: true,
  ...over,
});

describe("focusedPrReads", () => {
  it("reads a checkout the store knows nothing about, once for each half", () => {
    const seen = new Map();
    expect(
      focusedPrReads([pr({ number: undefined, checks: false, threads: false })], seen),
    ).toEqual({ live: ["fuji"], threads: ["fuji"] });
  });

  it("reads nothing for a warm checkout: the watcher's events keep it current", () => {
    const seen = new Map();
    expect(focusedPrReads([pr()], seen)).toEqual({ live: [], threads: [] });
    // Coming back into focus later is still nothing.
    expect(focusedPrReads([pr()], seen)).toEqual({ live: [], threads: [] });
  });

  it("owes an open PR its checks, but not a settled one", () => {
    expect(focusedPrReads([pr({ checks: false })], new Map()).live).toEqual(["fuji"]);
    expect(focusedPrReads([pr({ open: false, checks: false })], new Map()).live).toEqual([]);
  });

  it("reads a PR that opened under a checkout seen without one", () => {
    const seen = new Map();
    focusedPrReads([pr({ number: null, open: false })], seen);
    // `createPr` or a `pr:state_changed` brought the new PR: its checks and
    // threads are not in the store, and the watcher's seed emits nothing.
    expect(focusedPrReads([pr({ number: 651 })], seen)).toEqual({
      live: ["fuji"],
      threads: ["fuji"],
    });
    expect(focusedPrReads([pr({ number: 651 })], seen)).toEqual({ live: [], threads: [] });
  });

  it("does not take a PR going away for a new one", () => {
    const seen = new Map([["fuji", 650]]);
    expect(focusedPrReads([pr({ number: null, open: false })], seen)).toEqual({
      live: [],
      threads: [],
    });
  });

  it("decides per checkout", () => {
    const owed = focusedPrReads(
      [pr(), pr({ key: "fuji::api", number: undefined, checks: false, threads: false })],
      new Map(),
    );
    expect(owed).toEqual({ live: ["fuji::api"], threads: ["fuji::api"] });
  });
});

describe("prSignature", () => {
  const open = { number: 650, state: "open" } as PrState;

  it("round-trips what the store holds", () => {
    const maps = {
      prStates: { fuji: open, "fuji::api": null },
      prChecks: { fuji: null },
      prComments: {},
    };
    expect(parsePrSignature(prSignature(maps, "fuji"))).toEqual(
      pr({ checks: true, threads: false }),
    );
    expect(parsePrSignature(prSignature(maps, "fuji::api"))).toEqual(
      pr({ key: "fuji::api", number: null, open: false, checks: false, threads: false }),
    );
    expect(parsePrSignature(prSignature(maps, "kyoto"))).toEqual(
      pr({ key: "kyoto", number: undefined, open: false, checks: false, threads: false }),
    );
  });

  it("does not move when only the PR's detail does", () => {
    const before = { prStates: { fuji: open }, prChecks: { fuji: null }, prComments: {} };
    const after = { ...before, prStates: { fuji: { ...open, title: "renamed" } } };
    expect(prSignature(after, "fuji")).toBe(prSignature(before, "fuji"));
  });
});
