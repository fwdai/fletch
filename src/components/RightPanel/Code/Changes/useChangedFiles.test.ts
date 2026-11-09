import { describe, expect, it } from "vitest";
import type { FileStatus, TrackedRepo } from "@/api";
import { uncommittedFiles } from "./useChangedFiles";

const file = (path: string): FileStatus => ({
  path,
  kind: "modified",
  staged: false,
  additions: 1,
  deletions: 0,
});

const state = (...paths: string[]) => ({ files: paths.map(file) });
const repo = (subdir: string) => ({ subdir }) as TrackedRepo;

describe("uncommittedFiles", () => {
  it("is the primary checkout's list as is for a single-repo agent", () => {
    const primary = state("src/a.ts", "README.md");
    expect(uncommittedFiles([repo("app")], [primary])).toBe(primary.files);
  });

  it("is unknown, not empty, until the poll has answered", () => {
    expect(uncommittedFiles([repo("app")], [undefined])).toBeNull();
  });

  it("prefixes every checkout's paths with its directory for a multi-repo agent", () => {
    const files = uncommittedFiles(
      [repo("web"), repo("backend")],
      [state("src/a.ts"), state("src/b.rs")],
    );
    expect(files?.map((f) => f.path)).toEqual(["web/src/a.ts", "backend/src/b.rs"]);
  });

  it("stays unknown while any one checkout of a multi-repo agent is unanswered", () => {
    expect(
      uncommittedFiles([repo("web"), repo("backend")], [undefined, state("src/b.rs")]),
    ).toBeNull();
    expect(uncommittedFiles([repo("web"), repo("backend")], [state("src/a.ts")])).toBeNull();
  });

  it("is empty once every checkout has answered with nothing", () => {
    expect(uncommittedFiles([repo("web"), repo("backend")], [state(), state()])).toEqual([]);
  });
});
