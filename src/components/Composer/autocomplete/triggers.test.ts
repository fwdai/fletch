import { describe, expect, it } from "vitest";
import { triggerQueryAt, triggerTokenEnd } from "./triggers";

describe("triggerQueryAt (word anchor, e.g. @ and #)", () => {
  it("detects a token at the start of the text", () => {
    expect(triggerQueryAt("@src", 4, "@")).toEqual({ query: "src", start: 0 });
  });

  it("detects a token after whitespace", () => {
    expect(triggerQueryAt("look at @comp", 13, "@")).toEqual({ query: "comp", start: 8 });
  });

  it("matches an empty query right after the trigger", () => {
    expect(triggerQueryAt("hi @", 4, "@")).toEqual({ query: "", start: 3 });
  });

  it("ignores a trigger that is part of a word (e.g. an email)", () => {
    expect(triggerQueryAt("foo@bar", 7, "@")).toBeNull();
  });

  it("ends the token at whitespace, so a finished mention no longer fires", () => {
    expect(triggerQueryAt("@src/foo.ts done", 16, "@")).toBeNull();
  });

  it("uses the caret, not the end of the text", () => {
    expect(triggerQueryAt("@src more", 3, "@")).toEqual({ query: "sr", start: 0 });
  });

  it("works for the # trigger after whitespace", () => {
    expect(triggerQueryAt("see #12", 7, "#")).toEqual({ query: "12", start: 4 });
  });

  it("does not fire when the trigger butts against a preceding word", () => {
    // "@a#b": the # is preceded by "a", not whitespace, so it stays inert
    // (same rule that ignores emails like foo@bar).
    expect(triggerQueryAt("@a#b", 4, "#")).toBeNull();
  });
});

describe("triggerQueryAt (line-start anchor, e.g. / commands)", () => {
  it("fires at the start of the text", () => {
    expect(triggerQueryAt("/he", 3, "/", true)).toEqual({ query: "he", start: 0 });
  });

  it("fires at the start of a later line", () => {
    expect(triggerQueryAt("hi\n/he", 6, "/", true)).toEqual({ query: "he", start: 3 });
  });

  it("does NOT fire when the trigger is mid-line (after a space)", () => {
    expect(triggerQueryAt("hi /he", 6, "/", true)).toBeNull();
  });
});

describe("triggerTokenEnd", () => {
  it("scans to the end when the caret is mid-token", () => {
    // "@components", caret back at 4 — token ends at 11, so a pick removes
    // "ponents" too rather than leaving it behind.
    expect(triggerTokenEnd("@components", 4)).toBe(11);
  });

  it("stops at the first whitespace after the caret", () => {
    expect(triggerTokenEnd("@src more", 3)).toBe(4);
  });

  it("stops at a following trigger char (token boundary)", () => {
    expect(triggerTokenEnd("@a@b", 1)).toBe(2);
    expect(triggerTokenEnd("#1#2", 1)).toBe(2);
  });

  it("returns the caret when it already sits at the token end", () => {
    expect(triggerTokenEnd("@src", 4)).toBe(4);
  });
});
