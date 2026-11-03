import { describe, expect, it } from "vitest";
import { offersFolder, pathMentions } from "./mentions";

describe("pathMentions", () => {
  it("finds home and absolute paths written after an @", () => {
    expect(pathMentions("look at @~/Projects/lib and @/Volumes/Work/out please")).toEqual([
      "~/Projects/lib",
      "/Volumes/Work/out",
    ]);
    expect(pathMentions("@~/Projects/lib/")).toEqual(["~/Projects/lib/"]);
  });

  it("leaves sentence punctuation off the path", () => {
    expect(pathMentions("read @~/Projects/lib, then @~/notes.")).toEqual([
      "~/Projects/lib",
      "~/notes",
    ]);
  });

  it("names each path once", () => {
    expect(pathMentions("@~/a and again @~/a")).toEqual(["~/a"]);
  });

  it("ignores checkout mentions, relative paths and @ inside a word", () => {
    expect(pathMentions("@src/main.rs @./lib @../up me@/x ~/plain")).toEqual([]);
    expect(pathMentions("@~user")).toEqual([]);
  });
});

describe("offersFolder", () => {
  it("offers a folder below the top level", () => {
    expect(offersFolder("/Users/u/Projects/app", "~/Projects/app")).toBe(true);
    expect(offersFolder("/Users/u/Projects/app/", "/Users/u/Projects/app/")).toBe(true);
  });

  it("never offers the root, a top-level folder, or home as typed", () => {
    expect(offersFolder("/", "/")).toBe(false);
    expect(offersFolder("/Users", "/Users")).toBe(false);
    expect(offersFolder("/Users/u", "~")).toBe(false);
    expect(offersFolder("/Users/u/", "~/")).toBe(false);
  });
});
