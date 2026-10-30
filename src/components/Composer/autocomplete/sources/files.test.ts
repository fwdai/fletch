import { describe, expect, it } from "vitest";
import { folderRow } from "./files";

describe("folderRow", () => {
  it("offers the listed folder itself once the user is inside it", () => {
    expect(folderRow("/Users/u/Projects/app", "~/Projects/app", "", true)).toEqual({
      title: "app/",
      detail: "Give the agent this folder",
      icon: { glyph: "folder" },
    });
    // A trailing slash on the listing's base names the same folder.
    expect(folderRow("/Users/u/Projects/app/", "~/Projects/app/", "", true)?.title).toBe("app/");
  });

  it("stays out of the way while a name is being typed", () => {
    expect(folderRow("/Users/u/Projects", "~/Projects", "ap", true)).toBeNull();
  });

  it("is not offered where folders can't be attached", () => {
    expect(folderRow("/Users/u/Projects/app", "~/Projects/app", "", false)).toBeNull();
  });

  it("never offers the root, a top-level folder, or home as typed", () => {
    expect(folderRow("/", "/", "", true)).toBeNull();
    expect(folderRow("/Users", "/Users", "", true)).toBeNull();
    expect(folderRow("/Users/u", "~", "", true)).toBeNull();
    expect(folderRow("/Users/u", "~/", "", true)).toBeNull();
  });
});
