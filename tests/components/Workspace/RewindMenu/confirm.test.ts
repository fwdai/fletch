import { describe, expect, it } from "vitest";
import type { RepoRestore } from "@/api";
import { restoreConfirmation } from "@/components/Workspace/RewindMenu/confirm";

const repo = (subdir: string, over: Partial<RepoRestore> = {}): RepoRestore => ({
  subdir,
  branch: "main",
  checkpoint: "c0ffee",
  leaving: [],
  undo_ref: null,
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
    expect(confirmation.untouched).toEqual([]);
  });

  it("flags a branch whose leaving commits were already pushed", () => {
    const confirmation = restoreConfirmation("both", {
      repos: [repo("app", { leaving: [commit("b2", "local"), commit("a1", "shared", true)] })],
    });
    expect(confirmation.pushed).toBe(true);
  });

  it("names the checkouts with no snapshot, which stay as they are", () => {
    const confirmation = restoreConfirmation("code", {
      repos: [repo("app"), repo("added-later", { checkpoint: null })],
    });
    expect(confirmation.changes).toEqual([]);
    expect(confirmation.untouched).toEqual(["added-later"]);
  });
});
