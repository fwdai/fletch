import { describe, expect, it } from "vitest";
import type { PrState, TrackedRepo } from "@/api";
import { checkoutBase } from "./prState";

const repo = (over: Partial<TrackedRepo> = {}): TrackedRepo => ({
  repo_path: "/r",
  subdir: "repo",
  parent_branch: "main",
  pr_base: "feat/a",
  pr_number: 43,
  ...over,
});

const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 43,
  url: "u",
  state: "open",
  title: "t",
  mergeable: "unknown",
  base: "feat/a",
  ...over,
});

/** The base follows the checkout's own PR, never a PR that merely shares its
 *  branch name, and the polled Git state stands in until one is known. */
describe("checkoutBase", () => {
  it("prefers the bound PR's base over the polled Git state", () => {
    expect(checkoutBase(pr({ base: "develop" }), repo(), "main")).toBe("develop");
  });

  it("uses an open PR's base even before the record binds it", () => {
    expect(checkoutBase(pr({ number: 44, base: "develop" }), repo({ pr_number: 43 }), "main")).toBe(
      "develop",
    );
  });

  it("ignores a merged or closed PR the record does not bind", () => {
    const unbound = repo({ pr_number: null, pr_base: null });
    expect(checkoutBase(pr({ state: "merged", base: "develop" }), unbound, "main")).toBe("main");
    expect(checkoutBase(pr({ state: "closed", base: "develop" }), unbound)).toBe("main");
  });

  it("uses a merged PR's base when it is the bound one", () => {
    expect(checkoutBase(pr({ state: "merged", base: "develop" }), repo(), "main")).toBe("develop");
  });

  it("falls back from the poll to the record, then to nothing", () => {
    expect(checkoutBase(null, repo(), null)).toBe("feat/a");
    expect(checkoutBase(null, repo({ pr_base: null }))).toBe("main");
    expect(checkoutBase(null, undefined)).toBeNull();
  });
});
