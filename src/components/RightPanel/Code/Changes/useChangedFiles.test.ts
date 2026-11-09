import { describe, expect, it } from "vitest";
import type { FileStatus, GitState, TrackedRepo } from "@/api";
import { uncommittedFiles } from "./useChangedFiles";

const file = (path: string): FileStatus => ({
  path,
  kind: "modified",
  staged: false,
  additions: 1,
  deletions: 0,
});

const state = (...paths: string[]) => ({ files: paths.map(file) }) as GitState;
const repo = (subdir: string) => ({ subdir }) as TrackedRepo;

describe("uncommittedFiles", () => {
  it("is the primary checkout's list as is for a single-repo agent", () => {
    const primary = state("src/a.ts", "README.md");
    expect(uncommittedFiles([repo("app")], [primary])).toBe(primary.files);
  });

  it("is empty until the poll has answered", () => {
    expect(uncommittedFiles([repo("app")], [undefined])).toEqual([]);
  });

  it("prefixes every checkout's paths with its directory for a multi-repo agent", () => {
    const files = uncommittedFiles(
      [repo("web"), repo("backend")],
      [state("src/a.ts"), state("src/b.rs")],
    );
    expect(files.map((f) => f.path)).toEqual(["web/src/a.ts", "backend/src/b.rs"]);
  });

  it("skips a checkout whose state hasn't arrived without dropping the others", () => {
    const files = uncommittedFiles([repo("web"), repo("backend")], [undefined, state("src/b.rs")]);
    expect(files.map((f) => f.path)).toEqual(["backend/src/b.rs"]);
  });
});
