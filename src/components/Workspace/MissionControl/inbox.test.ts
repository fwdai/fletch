import { describe, expect, it } from "vitest";
import type { TrackerIssue } from "@/api";
import {
  branchKind,
  composeIssueBrief,
  deriveInboxRows,
  slugifyTitle,
  suggestBranchName,
} from "./inbox";

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

function linearIssue(over: Partial<TrackerIssue> = {}): TrackerIssue {
  return issue({
    source: "linear",
    key: "ENG-1",
    url: "https://linear.app/acme/issue/ENG-1",
    ...over,
  });
}

describe("slugifyTitle", () => {
  it("lowercases, dashes non-alphanumerics, and clamps words", () => {
    expect(slugifyTitle("Login crashes on empty password!")).toBe(
      "login-crashes-on-empty-password",
    );
    expect(slugifyTitle("A B C D E F G", 3)).toBe("a-b-c");
  });

  it("collapses runs and trims trailing dashes", () => {
    expect(slugifyTitle("  Fix   the -- thing  ")).toBe("fix-the-thing");
    expect(slugifyTitle("!!!")).toBe("");
  });
});

describe("branchKind", () => {
  it("infers feat / chore / fix from labels", () => {
    expect(branchKind([{ name: "enhancement" }])).toBe("feat");
    expect(branchKind([{ name: "documentation" }])).toBe("chore");
    expect(branchKind([{ name: "bug" }])).toBe("fix");
    expect(branchKind([])).toBe("fix");
  });
});

describe("suggestBranchName", () => {
  it("builds kind/key-slug", () => {
    expect(suggestBranchName(issue({ key: "123", title: "Login crash" }))).toBe(
      "fix/123-login-crash",
    );
    expect(
      suggestBranchName(issue({ key: "9", title: "Add dark mode", labels: [{ name: "feature" }] })),
    ).toBe("feat/9-add-dark-mode");
  });

  it("lowercases a tracker key", () => {
    expect(suggestBranchName(linearIssue({ key: "ENG-123", title: "Login crash" }))).toBe(
      "fix/eng-123-login-crash",
    );
  });

  it("falls back to the key when the title has no slug", () => {
    expect(suggestBranchName(issue({ key: "7", title: "🎉🎉🎉" }))).toBe("fix/7");
  });
});

describe("composeIssueBrief", () => {
  it("includes reference, body, url, and the branch suggestion", () => {
    const brief = composeIssueBrief(
      issue({ key: "42", title: "Crash on save", body: "Steps:\n1. save", url: "https://x/42" }),
    );
    expect(brief).toContain("GitHub issue #42: Crash on save");
    expect(brief).toContain("Steps:\n1. save");
    expect(brief).toContain("https://x/42");
    expect(brief).toContain("`fix/42-crash-on-save`");
  });

  it("speaks the source's reference form for Linear", () => {
    const brief = composeIssueBrief(linearIssue({ key: "ENG-42", title: "Crash on save" }));
    expect(brief).toContain("Linear issue ENG-42: Crash on save");
    expect(brief).toContain("`fix/eng-42-crash-on-save`");
  });

  it("omits an empty body block", () => {
    const brief = composeIssueBrief(issue({ key: "5", title: "T", body: "  " }));
    expect(brief).not.toContain("\n\n\n");
  });
});

describe("deriveInboxRows", () => {
  it("merges repos, keys by repo+source+key, and sorts newest-updated first", () => {
    const rows = deriveInboxRows([
      {
        repoPath: "/a",
        repoLabel: "A",
        issues: [
          issue({ key: "1", updated_at: 100 }),
          linearIssue({ key: "ENG-2", updated_at: 300 }),
        ],
      },
      { repoPath: "/b", repoLabel: "B", issues: [issue({ key: "1", updated_at: 200 })] },
    ]);
    expect(rows.map((r) => r.key)).toEqual(["/a#linear:ENG-2", "/b#github:1", "/a#github:1"]);
  });

  it("sorts issues without a timestamp last and respects the limit", () => {
    const rows = deriveInboxRows(
      [
        {
          repoPath: "/a",
          repoLabel: "A",
          issues: [issue({ key: "1" }), issue({ key: "2", updated_at: 5 })],
        },
      ],
      1,
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].key).toBe("/a#github:2");
  });
});
