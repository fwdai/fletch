import { describe, expect, it } from "vitest";
import type { RepoRestore } from "@/api";
import { restoreConfirmation } from "@/components/Workspace/RewindMenu/confirm";

const repo = (subdir: string, over: Partial<RepoRestore> = {}): RepoRestore => ({
  subdir,
  branch: "main",
  checkpoint: "c0ffee",
  leaving: [],
  ...over,
});

const commit = (sha: string, subject: string, pushed = false) => ({ sha, subject, pushed });

describe("the code restore confirmation", () => {
  it("names what is restored", () => {
    const code = restoreConfirmation("code", { repos: [repo("app")] });
    expect([code.title, code.action]).toEqual(["Restore the code?", "Restore code"]);
    const both = restoreConfirmation("both", { repos: [repo("app")] });
    expect([both.title, both.action]).toEqual([
      "Restore the conversation and code?",
      "Restore conversation and code",
    ]);
  });

  it("lists, per checkout, the commits that leave its branch", () => {
    const confirmation = restoreConfirmation("code", {
      repos: [
        repo("app", { leaving: [commit("b2", "second"), commit("a1", "first")] }),
        repo("docs"),
        repo("api", { branch: null, leaving: [commit("c3", "detached work")] }),
      ],
    });

    expect(confirmation.changes).toEqual([
      { subdir: "app", branch: "main", leaving: [commit("b2", "second"), commit("a1", "first")] },
      { subdir: "api", branch: null, leaving: [commit("c3", "detached work")] },
    ]);
    expect(confirmation.pushed).toBe(false);
    expect(confirmation.keptAsIs).toEqual([]);
    expect(confirmation.summary).toBe(
      "Each checkout goes back to how it was when this message was sent.",
    );
  });

  it("flags a branch whose leaving commits were already pushed", () => {
    const confirmation = restoreConfirmation("both", {
      repos: [repo("app", { leaving: [commit("b2", "local"), commit("a1", "shared", true)] })],
    });
    expect(confirmation.pushed).toBe(true);
  });

  it("names, one row each, the checkouts a partial restore leaves as they are", () => {
    const confirmation = restoreConfirmation("code", {
      repos: [
        repo("repo-a", { leaving: [commit("b2", "since")] }),
        repo("repo-b", { checkpoint: null }),
        repo("repo-c", { checkpoint: null }),
      ],
    });

    expect(confirmation.keptAsIs).toEqual([
      "repo-b: no snapshot of this message, so it stays as it is now",
      "repo-c: no snapshot of this message, so it stays as it is now",
    ]);
    expect(confirmation.summary).toMatch(/^Only part of the code goes back/);
    expect(confirmation.changes.map((change) => change.subdir)).toEqual(["repo-a"]);
  });
});
