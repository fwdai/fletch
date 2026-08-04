import { describe, expect, it } from "vitest";
import type { RoadmapItem, TrackerIssue } from "@/api";
import {
  composeIssueWhy,
  distillIssueBody,
  funnelAction,
  issueToRoadmapItem,
  routedIssueUrls,
} from "./funnel";

function issue(over: Partial<TrackerIssue> = {}): TrackerIssue {
  return {
    source: "github",
    key: "1",
    title: "An issue",
    url: "https://github.com/o/r/issues/1",
    labels: [],
    ...over,
  };
}

function row(why: string): Pick<RoadmapItem, "why"> {
  return { why };
}

describe("distillIssueBody", () => {
  it("collapses whitespace and drops template comments", () => {
    expect(distillIssueBody("<!-- describe it -->\nLogin  fails\n\non save")).toBe(
      "Login fails on save",
    );
  });

  it("is empty for a missing or blank body", () => {
    expect(distillIssueBody(undefined)).toBe("");
    expect(distillIssueBody("  \n\n")).toBe("");
    expect(distillIssueBody("<!-- only guidance -->")).toBe("");
  });

  it("clips at a word boundary with an ellipsis", () => {
    const clipped = distillIssueBody("alpha beta gamma delta", 12);
    expect(clipped).toBe("alpha beta…");
  });

  it("hard-clips a single long word rather than losing everything", () => {
    expect(distillIssueBody("aaaaaaaaaaaaaaaa", 8)).toBe("aaaaaaaa…");
  });
});

describe("composeIssueWhy", () => {
  it("puts the url alone on the first line, body under it", () => {
    const why = composeIssueWhy(issue({ url: "https://x/42", body: "Steps:\n1. save" }));
    expect(why).toBe("https://x/42\nSteps: 1. save");
    expect(why.split("\n")[0]).toBe("https://x/42");
  });

  it("is just the url when the body is empty", () => {
    expect(composeIssueWhy(issue({ url: "https://x/42", body: "  " }))).toBe("https://x/42");
    expect(composeIssueWhy(issue({ url: "https://x/42" }))).toBe("https://x/42");
  });
});

describe("issueToRoadmapItem", () => {
  it("lands a GitHub issue as a proposed ghost with the github source", () => {
    expect(issueToRoadmapItem(issue({ title: "Crash on save", url: "https://x/1" }))).toEqual({
      title: "Crash on save",
      why: "https://x/1",
      status: "proposed",
      source: "github",
    });
  });

  it("maps a Linear ticket to the linear source", () => {
    const item = issueToRoadmapItem(
      issue({ source: "linear", key: "ENG-9", url: "https://linear.app/acme/issue/ENG-9" }),
    );
    expect(item.source).toBe("linear");
    expect(item.status).toBe("proposed");
  });

  it("leaves horizon and rank to the backend's defaults", () => {
    const item = issueToRoadmapItem(issue());
    expect(item.horizon).toBeUndefined();
    expect(item.accept).toBeUndefined();
  });
});

describe("routedIssueUrls", () => {
  it("reads the url off each row's first line", () => {
    const urls = routedIssueUrls([
      row("https://github.com/o/r/issues/1\nLogin fails"),
      row("https://linear.app/acme/issue/ENG-2"),
    ]);
    expect([...urls].sort()).toEqual([
      "https://github.com/o/r/issues/1",
      "https://linear.app/acme/issue/ENG-2",
    ]);
  });

  it("ignores a why that doesn't open with a bare url", () => {
    expect(
      routedIssueUrls([
        row("Because users keep asking"),
        row(""),
        row("see https://github.com/o/r/issues/9 for detail"),
      ]).size,
    ).toBe(0);
  });
});

describe("funnelAction", () => {
  const routed = new Set(["https://github.com/o/r/issues/1"]);

  it("offers no action when the repo belongs to no project", () => {
    expect(funnelAction(undefined, "https://github.com/o/r/issues/2", routed)).toEqual({
      kind: "none",
    });
    expect(funnelAction("", "https://github.com/o/r/issues/1", routed)).toEqual({ kind: "none" });
  });

  it("reports an already-routed issue", () => {
    expect(funnelAction("p1", "https://github.com/o/r/issues/1", routed)).toEqual({
      kind: "routed",
    });
  });

  it("offers the add carrying the project to create in", () => {
    expect(funnelAction("p1", "https://github.com/o/r/issues/2", routed)).toEqual({
      kind: "add",
      projectId: "p1",
    });
  });
});
