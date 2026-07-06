import { describe, expect, it } from "vitest";

import { moveInOrder, sortByOrder } from "@/storage/projectOrder";

describe("sortByOrder", () => {
  it("orders paths by their index in the saved order", () => {
    expect(sortByOrder(["/a", "/b", "/c"], ["/c", "/a", "/b"])).toEqual(["/c", "/a", "/b"]);
  });

  it("appends paths missing from the saved order, keeping their natural order", () => {
    // /b and /c are known; /new and /new2 are unseen and sort last, in order.
    expect(sortByOrder(["/new", "/b", "/new2", "/c"], ["/c", "/b"])).toEqual([
      "/c",
      "/b",
      "/new",
      "/new2",
    ]);
  });

  it("falls back to natural order when nothing is saved", () => {
    expect(sortByOrder(["/a", "/b"], [])).toEqual(["/a", "/b"]);
  });

  it("ignores saved paths that no longer exist", () => {
    expect(sortByOrder(["/a", "/b"], ["/gone", "/b", "/a"])).toEqual(["/b", "/a"]);
  });
});

describe("moveInOrder", () => {
  it("drops after the target when dragging downward", () => {
    expect(moveInOrder(["/a", "/b", "/c", "/d"], "/a", "/c")).toEqual(["/b", "/c", "/a", "/d"]);
  });

  it("drops before the target when dragging upward", () => {
    expect(moveInOrder(["/a", "/b", "/c", "/d"], "/d", "/b")).toEqual(["/a", "/d", "/b", "/c"]);
  });

  it("is a no-op when source and target are the same", () => {
    expect(moveInOrder(["/a", "/b"], "/a", "/a")).toEqual(["/a", "/b"]);
  });

  it("returns the input unchanged when a path is missing", () => {
    expect(moveInOrder(["/a", "/b"], "/x", "/b")).toEqual(["/a", "/b"]);
  });
});
