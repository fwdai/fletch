import { describe, expect, it } from "vitest";
import { spliceTranscript } from "./spliceTranscript";

describe("spliceTranscript", () => {
  it("uses the transcript alone when the box was empty", () => {
    expect(spliceTranscript("", "hello there")).toEqual({ text: "hello there", caret: 11 });
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

  it("lands the caret at the end of the spliced text", () => {
    const { text, caret } = spliceTranscript("draft ", "hello world");
    expect(caret).toBe(text.length);
  });
});
