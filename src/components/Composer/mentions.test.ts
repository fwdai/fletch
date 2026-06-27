import { describe, expect, it } from "vitest";
import { filterDirEntries, filterFiles, isFsPath, joinTypedDir, splitFsPath } from "./mentions";

describe("filterFiles", () => {
  const files = [
    "src/components/Composer/index.tsx",
    "src/components/Composer/SlashMenu.tsx",
    "src/api.ts",
    "src/store.ts",
    "README.md",
  ];

  it("returns the first N files for an empty query", () => {
    expect(filterFiles(files, "", 2)).toEqual(files.slice(0, 2));
  });

  it("ranks a basename prefix match first", () => {
    expect(filterFiles(files, "index")[0]).toBe("src/components/Composer/index.tsx");
  });

  it("matches anywhere in the path", () => {
    expect(filterFiles(files, "composer")).toContain("src/components/Composer/SlashMenu.tsx");
  });

  it("falls back to a fuzzy subsequence match", () => {
    expect(filterFiles(files, "slmnu")).toContain("src/components/Composer/SlashMenu.tsx");
  });

  it("excludes non-matches", () => {
    expect(filterFiles(files, "xyzzy")).toEqual([]);
  });

  it("honors the result limit", () => {
    expect(filterFiles(files, "s").length).toBeLessThanOrEqual(8);
  });
});

describe("isFsPath", () => {
  it.each(["~", "~/Downloads", "/etc", "./src", "../x"])("treats %s as a filesystem path", (q) =>
    expect(isFsPath(q)).toBe(true));
  it.each(["src", "Composer", "index.ts", ""])("treats %s as a worktree search", (q) =>
    expect(isFsPath(q)).toBe(false));
});

describe("splitFsPath", () => {
  it("splits a tilde root", () => {
    expect(splitFsPath("~/Down")).toEqual({ dir: "~", partial: "Down" });
  });
  it("keeps the bare prefix when there's no slash", () => {
    expect(splitFsPath("~")).toEqual({ dir: "~", partial: "" });
  });
  it("treats the filesystem root specially", () => {
    expect(splitFsPath("/Us")).toEqual({ dir: "/", partial: "Us" });
  });
  it("splits a nested path", () => {
    expect(splitFsPath("/Users/alex/Do")).toEqual({
      dir: "/Users/alex",
      partial: "Do",
    });
  });
});

describe("joinTypedDir", () => {
  it.each([
    ["~", "Downloads", "~/Downloads"],
    ["/", "Users", "/Users"],
    ["~/Downloads", "img", "~/Downloads/img"],
    ["~/Downloads/", "img", "~/Downloads/img"],
  ])("joins %s + %s", (dir, name, expected) => {
    expect(joinTypedDir(dir, name)).toBe(expected);
  });
});

describe("filterDirEntries", () => {
  const entries = [
    { name: "Documents", is_dir: true },
    { name: "archive.zip", is_dir: false },
    { name: "1.png", is_dir: false },
    { name: ".hidden", is_dir: false },
    { name: "apps", is_dir: true },
  ];

  it("lists directories before files", () => {
    const out = filterDirEntries(entries, "");
    const firstFile = out.findIndex((e) => !e.is_dir);
    const lastDir = out.map((e) => e.is_dir).lastIndexOf(true);
    expect(lastDir).toBeLessThan(firstFile);
  });

  it("hides dotfiles unless the partial starts with a dot", () => {
    expect(filterDirEntries(entries, "").some((e) => e.name === ".hidden")).toBe(false);
    expect(filterDirEntries(entries, ".").map((e) => e.name)).toContain(".hidden");
  });

  it("filters by case-insensitive substring", () => {
    expect(filterDirEntries(entries, "png").map((e) => e.name)).toEqual(["1.png"]);
  });
});
