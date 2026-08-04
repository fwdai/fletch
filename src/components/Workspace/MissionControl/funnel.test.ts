import { describe, expect, it } from "vitest";
import type { RoadmapItem, TrackerIssue } from "@/api";
import {
  composeIssueWhy,
  distillIssueBody,
  funnelAction,
  issueToRoadmapItem,
  parseDeclinedIssues,
  routedIssueUrl,
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

/** A row as the funnel reads it. `issue_url` defaults to null so the legacy
 *  why-line path is what a bare `row("…")` exercises. */
function row(why: string, issue_url: string | null = null): Pick<RoadmapItem, "why" | "issue_url"> {
  return { why, issue_url };
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

  it("spends the budget on prose, not screenshots, code fences or link urls", () => {
    const body =
      "![screenshot](https://user-images.example/a-very-long-signed-url-that-eats-the-budget.png)\n" +
      "```\nthread 'main' panicked at src/lib.rs:42\n```\n" +
      "Saving a note [drops the body](https://github.com/o/r/issues/7#issuecomment-1) every time.";
    expect(distillIssueBody(body)).toBe("Saving a note drops the body every time.");
  });

  it("is empty for a body that is only markdown noise", () => {
    expect(distillIssueBody("![shot](https://x/a.png)\n\n```\nlet x = 1;\n```")).toBe("");
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
      issue_url: "https://x/1",
    });
  });

  // The durable routing record, and the only place it is ever written.
  it("carries the issue url as a field, not only as prose", () => {
    const item = issueToRoadmapItem(issue({ url: "https://x/42", body: "Steps" }));
    expect(item.issue_url).toBe("https://x/42");
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

describe("routedIssueUrl", () => {
  it("reads the column, whatever the prose says", () => {
    // The whole point of migration 0036: the `why` is the user's to rewrite (and
    // the PM's to propose changes to), and dedup must not notice.
    expect(routedIssueUrl(row("Because three people asked", "https://x/7"))).toBe("https://x/7");
    expect(routedIssueUrl(row("", "https://x/7"))).toBe("https://x/7");
  });

  it("falls back to the first line for rows written before the column existed", () => {
    expect(routedIssueUrl(row("https://github.com/o/r/issues/1\nLogin fails"))).toBe(
      "https://github.com/o/r/issues/1",
    );
    expect(routedIssueUrl(row("https://linear.app/acme/issue/ENG-2"))).toBe(
      "https://linear.app/acme/issue/ENG-2",
    );
  });

  it("is null for a row nobody imported", () => {
    expect(routedIssueUrl(row("Because users keep asking"))).toBeNull();
    expect(routedIssueUrl(row(""))).toBeNull();
    expect(routedIssueUrl(row("see https://github.com/o/r/issues/9 for detail"))).toBeNull();
  });
});

describe("routedIssueUrls", () => {
  it("collects both the column and the legacy first line", () => {
    const urls = routedIssueUrls([
      row("Rewritten rationale", "https://github.com/o/r/issues/1"),
      row("https://linear.app/acme/issue/ENG-2"),
      row("Not from a tracker at all"),
    ]);
    expect([...urls].sort()).toEqual([
      "https://github.com/o/r/issues/1",
      "https://linear.app/acme/issue/ENG-2",
    ]);
  });

  it("is empty for a board of hand-written rows", () => {
    expect(routedIssueUrls([row("Because users keep asking"), row("")]).size).toBe(0);
  });
});

describe("parseDeclinedIssues", () => {
  it("reads the stored JSON array", () => {
    expect([...parseDeclinedIssues('["https://x/1","https://x/2"]')]).toEqual([
      "https://x/1",
      "https://x/2",
    ]);
  });

  it("is empty for anything it can't trust", () => {
    for (const value of [undefined, "", "not json", "{}", '"a string"', "[1,2]"]) {
      expect(parseDeclinedIssues(value).size, String(value)).toBe(0);
    }
  });

  it("keeps only the strings out of a mixed array", () => {
    expect([...parseDeclinedIssues('["https://x/1",7,null]')]).toEqual(["https://x/1"]);
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

  it("offers no action for an issue with no url to dedup on", () => {
    expect(funnelAction("p1", "", routed)).toEqual({ kind: "none" });
  });

  // The durable-refusal half: a discarded ghost used to be re-offered on every
  // read, forever, because the record of the refusal died with the row.
  it("reports an issue the user turned down instead of offering it again", () => {
    const declined = new Set(["https://github.com/o/r/issues/5"]);
    expect(funnelAction("p1", "https://github.com/o/r/issues/5", routed, declined)).toEqual({
      kind: "declined",
    });
    // Untouched issues are still addable.
    expect(funnelAction("p1", "https://github.com/o/r/issues/6", routed, declined)).toEqual({
      kind: "add",
      projectId: "p1",
    });
  });

  it("lets the board outrank the tombstone when an issue was routed again", () => {
    const declined = new Set(["https://github.com/o/r/issues/1"]);
    expect(funnelAction("p1", "https://github.com/o/r/issues/1", routed, declined)).toEqual({
      kind: "routed",
    });
  });

  it("treats an unknown declined set as nothing declined", () => {
    expect(funnelAction("p1", "https://github.com/o/r/issues/2", routed)).toEqual({
      kind: "add",
      projectId: "p1",
    });
  });

  // The same origin repo can be pinned in two projects; each board is judged
  // against its own routed set, so routing in one leaves the other addable.
  it("scopes the routed decision to the project's own board", () => {
    const routedByProject: Record<string, Set<string>> = { p1: routed, p2: new Set() };
    expect(funnelAction("p1", "https://github.com/o/r/issues/1", routedByProject.p1)).toEqual({
      kind: "routed",
    });
    expect(funnelAction("p2", "https://github.com/o/r/issues/1", routedByProject.p2)).toEqual({
      kind: "add",
      projectId: "p2",
    });
  });
});
