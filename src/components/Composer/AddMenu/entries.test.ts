import { describe, expect, it } from "vitest";
import type { PrSummary, RoadmapItem, TrackerIssue } from "@/api";
import { issueEntries, prEntries, roadmapEntries } from "./entries";

const issue = (source: TrackerIssue["source"], key: string): TrackerIssue => ({
  source,
  key,
  title: `Issue ${key}`,
  url: "",
  labels: [],
});

const item = (code: string, patch: Partial<RoadmapItem> = {}) =>
  ({
    id: `id-${code}`,
    code,
    title: `Item ${code}`,
    horizon: "now",
    status: "open",
    agent_id: null,
    ...patch,
  }) as RoadmapItem;

describe("issueEntries", () => {
  it("groups by tracker and keeps each tracker's own order", () => {
    const rows = issueEntries([
      issue("linear", "ENG-2"),
      issue("github", "12"),
      issue("linear", "ENG-1"),
      issue("github", "7"),
    ]);
    expect(rows.map((r) => [r.group, r.tag])).toEqual([
      ["GitHub", "#12"],
      ["GitHub", "#7"],
      ["Linear", "ENG-2"],
      ["Linear", "ENG-1"],
    ]);
  });
});

describe("prEntries", () => {
  it("lists the newest pull request first", () => {
    const prs = [{ number: 3 }, { number: 11 }, { number: 7 }] as PrSummary[];
    expect(prEntries(prs).map((r) => r.tag)).toEqual(["#11", "#7", "#3"]);
  });
});

describe("roadmapEntries", () => {
  it("offers only agreed work nobody has yet", () => {
    const rows = roadmapEntries([
      item("A-1"),
      item("A-2", { status: "proposed" }),
      item("A-3", { status: "queued" }),
      item("A-4", { status: "active" }),
      item("A-5", { status: "done" }),
      item("A-6", { agent_id: "alps" }),
    ]);
    expect(rows.map((r) => r.tag)).toEqual(["A-1"]);
  });

  it("orders by horizon, keeping board order within one", () => {
    const rows = roadmapEntries([
      item("L-1", { horizon: "later" }),
      item("N-1", { horizon: "now" }),
      item("X-1", { horizon: "next" }),
      item("N-2", { horizon: "now" }),
    ]);
    expect(rows.map((r) => [r.group, r.tag])).toEqual([
      ["Now", "N-1"],
      ["Now", "N-2"],
      ["Next", "X-1"],
      ["Later", "L-1"],
    ]);
  });
});
