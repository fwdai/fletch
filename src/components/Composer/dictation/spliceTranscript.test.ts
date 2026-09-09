import { describe, expect, it } from "vitest";
import { insertTranscript, spliceTranscript, splitForTranscript } from "./spliceTranscript";

describe("spliceTranscript", () => {
  it("uses the transcript alone when the box was empty", () => {
    expect(spliceTranscript("", "hello there")).toEqual({
      text: "hello there",
      start: 0,
      caret: 11,
    });
  });

  it("adds the missing word boundary after typed text", () => {
    expect(spliceTranscript("draft", "hello").text).toBe("draft hello");
  });

  it("keeps the user's own trailing space rather than doubling it", () => {
    expect(spliceTranscript("draft ", "hello").text).toBe("draft hello");
  });

  it("keeps a trailing newline, so a dictated line starts where they left off", () => {
    expect(spliceTranscript("- one\n", "two").text).toBe("- one\ntwo");
  });

  it("replaces, never appends — a revised transcript reuses the same base", () => {
    const base = "draft";
    expect(spliceTranscript(base, "hello wold").text).toBe("draft hello wold");
    expect(spliceTranscript(base, "hello world").text).toBe("draft hello world");
  });

  it("leaves the base untouched while nothing has been heard", () => {
    expect(spliceTranscript("draft", "").text).toBe("draft");
    expect(spliceTranscript("", "")).toEqual({ text: "", start: 0, caret: 0 });
  });

  it("lands the caret at the end of the spliced text", () => {
    const { text, caret } = spliceTranscript("draft ", "hello world");
    expect(caret).toBe(text.length);
  });

  it("marks where the transcript starts, after the added boundary", () => {
    expect(spliceTranscript("draft", "hello").start).toBe("draft ".length);
    expect(spliceTranscript("draft ", "hello").start).toBe("draft ".length);
  });
});

describe("insertTranscript", () => {
  it("inserts at the caret, adding a boundary on each side that lacks one", () => {
    expect(insertTranscript("one two", 3, "and")).toEqual({
      text: "one and two",
      start: 4,
      caret: 7,
    });
  });

  it("adds no boundary where the user already left one", () => {
    expect(insertTranscript("one  two", 4, "and").text).toBe("one and two");
    expect(insertTranscript("one\ntwo", 4, "and").text).toBe("one\nand two");
  });

  it("puts the caret after the dictated words, before the trailing text", () => {
    const { text, caret } = insertTranscript("one two", 3, "and");
    expect(text.slice(caret)).toBe(" two");
  });

  it("clamps a caret outside the text", () => {
    expect(insertTranscript("one", 99, "two").text).toBe("one two");
    expect(insertTranscript("one", -1, "two").text).toBe("two one");
  });

  it("changes nothing when nothing was heard, whatever the caret", () => {
    expect(insertTranscript("one two", 3, "")).toEqual({ text: "one two", start: 3, caret: 3 });
  });
});

describe("splitForTranscript", () => {
  it("previews the same join the insert will make", () => {
    const { before, lead, trail, after } = splitForTranscript("one two", 3, "and");
    expect(`${before}${lead}and${trail}${after}`).toBe(insertTranscript("one two", 3, "and").text);
  });
});
