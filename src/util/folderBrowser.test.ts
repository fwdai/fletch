import { describe, expect, it } from "vitest";
import type { DirListing } from "@/api/types/checkout";
import { folderView } from "./folderBrowser";

const listing: DirListing = {
  base: "/Users/alex",
  entries: [
    { name: "Downloads", is_dir: true },
    { name: ".zshrc", is_dir: false },
    { name: "Code", is_dir: true, is_repo: false },
    { name: ".config", is_dir: true },
    { name: "README.md", is_dir: false },
    { name: "fletch", is_dir: true, is_repo: true },
  ],
};

describe("folderView", () => {
  it("shows the directories only, sorted, with the hidden ones held back", () => {
    const view = folderView(listing, false);

    expect(view.shown.map((e) => e.name)).toEqual(["Code", "Downloads", "fletch"]);
    expect(view.hiddenCount).toBe(1);
  });

  it("adds the hidden folders in order when they are asked for", () => {
    expect(folderView(listing, true).shown.map((e) => e.name)).toEqual([
      ".config",
      "Code",
      "Downloads",
      "fletch",
    ]);
  });

  it("is empty before anything has been read", () => {
    expect(folderView(null, true)).toEqual({ shown: [], hiddenCount: 0 });
  });
});
