import { describe, expect, it } from "vitest";
import type { IssueSummary } from "@/api";
import {
  branchKind,
  composeIssueBrief,
  deriveInboxRows,
  slugifyTitle,
  suggestBranchName,
} from "./inbox";

function issue(over: Partial<IssueSummary> = {}): IssueSummary {
  return {
    number: 1,
    title: "An issue",
    url: "https://github.com/o/r/issues/1",
    labels: [],
    ...over,
  };
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
  it("builds kind/number-slug", () => {
    expect(suggestBranchName(issue({ number: 123, title: "Login crash" }))).toBe(
      "fix/123-login-crash",
    );
    expect(
      suggestBranchName(
        issue({ number: 9, title: "Add dark mode", labels: [{ name: "feature" }] }),
      ),
    ).toBe("feat/9-add-dark-mode");
  });

  it("falls back to the number when the title has no slug", () => {
    expect(suggestBranchName(issue({ number: 7, title: "🎉🎉🎉" }))).toBe("fix/7");
  });
});

describe("composeIssueBrief", () => {
  it("includes reference, body, url, and the branch suggestion", () => {
    const brief = composeIssueBrief(
      issue({ number: 42, title: "Crash on save", body: "Steps:\n1. save", url: "https://x/42" }),
    );
    expect(brief).toContain("GitHub issue #42: Crash on save");
    expect(brief).toContain("Steps:\n1. save");
    expect(brief).toContain("https://x/42");
    expect(brief).toContain("`fix/42-crash-on-save`");
  });

  it("omits an empty body block", () => {
    const brief = composeIssueBrief(issue({ number: 5, title: "T", body: "  " }));
    expect(brief).not.toContain("\n\n\n");
  });
});

describe("deriveInboxRows", () => {
  it("merges repos, keys by repo+number, and sorts newest-updated first", () => {
    const rows = deriveInboxRows([
      {
        repoPath: "/a",
        repoLabel: "A",
        issues: [issue({ number: 1, updated_at: 100 }), issue({ number: 2, updated_at: 300 })],
      },
      { repoPath: "/b", repoLabel: "B", issues: [issue({ number: 1, updated_at: 200 })] },
    ]);
    expect(rows.map((r) => r.key)).toEqual(["/a#2", "/b#1", "/a#1"]);
  });

  it("sorts issues without a timestamp last and respects the limit", () => {
    const rows = deriveInboxRows(
      [
        {
          repoPath: "/a",
          repoLabel: "A",
          issues: [issue({ number: 1 }), issue({ number: 2, updated_at: 5 })],
        },
      ],
      1,
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].key).toBe("/a#2");
  });
});
