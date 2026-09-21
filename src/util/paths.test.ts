import { describe, expect, it } from "vitest";
import { baseName, childPath, parentPath } from "./paths";

describe("childPath", () => {
  it("joins without doubling the separator the root's base carries", () => {
    expect(childPath("/Users/alex", "Code")).toBe("/Users/alex/Code");
    expect(childPath("/Users/alex/", "Code")).toBe("/Users/alex/Code");
    expect(childPath("/", "Users")).toBe("/Users");
  });
});

describe("parentPath", () => {
  it("walks up, and stops at the root", () => {
    expect(parentPath("/Users/alex/Code")).toBe("/Users/alex");
    expect(parentPath("/Users/alex/")).toBe("/Users");
    expect(parentPath("/Users")).toBe("/");
    expect(parentPath("/")).toBeNull();
  });

  it("has no parent for a path with no separator at all", () => {
    expect(parentPath("~")).toBeNull();
  });
});

describe("baseName", () => {
  it("is the last segment, trailing separator or not", () => {
    expect(baseName("/Users/alex/Code")).toBe("Code");
    expect(baseName("/Users/alex/Code/")).toBe("Code");
    expect(baseName("/")).toBe("/");
  });
});
