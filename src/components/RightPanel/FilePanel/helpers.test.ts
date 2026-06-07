import { describe, expect, it } from "vitest";
import { buildTree, duplicatePath } from "./tree";
import { joinPath, parentDir } from "../../../util/format";
import type { WorktreeFile } from "../../../api";

const f = (path: string): WorktreeFile => ({ path, status: null, additions: 0, deletions: 0 });

describe("parentDir / joinPath", () => {
  it("splits and rejoins paths, treating root as empty", () => {
    expect(parentDir("a/b/c.ts")).toBe("a/b");
    expect(parentDir("top.ts")).toBe("");
    expect(joinPath("a/b", "c.ts")).toBe("a/b/c.ts");
    expect(joinPath("", "top.ts")).toBe("top.ts");
  });
});

describe("duplicatePath", () => {
  it("inserts ' copy' before the extension", () => {
    expect(duplicatePath("a/foo.ts", new Set(["a/foo.ts"]))).toBe("a/foo copy.ts");
  });

  it("increments when the copy already exists", () => {
    const existing = new Set(["foo.ts", "foo copy.ts", "foo copy 2.ts"]);
    expect(duplicatePath("foo.ts", existing)).toBe("foo copy 3.ts");
  });

  it("handles extensionless and dotfile names", () => {
    expect(duplicatePath("README", new Set(["README"]))).toBe("README copy");
    expect(duplicatePath(".gitignore", new Set([".gitignore"]))).toBe(".gitignore copy");
  });
});

describe("buildTree", () => {
  it("injects empty extra directories so new folders survive a refresh", () => {
    const tree = buildTree([f("src/app.ts")], ["src/empty"]);
    const src = tree.find((n) => n.path === "src");
    expect(src?.type).toBe("dir");
    if (src?.type !== "dir") throw new Error("expected dir");
    // dirs sort before files: empty/ then app.ts
    expect(src.children.map((c) => c.path)).toEqual(["src/empty", "src/app.ts"]);
    expect(src.children[0].type).toBe("dir");
  });
});
