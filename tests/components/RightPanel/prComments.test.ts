import { describe, expect, it } from "vitest";
import type { PrComment } from "@/api";
import { commentLocation, formatCommentForChat } from "@/components/RightPanel/prComments";

const base: PrComment = {
  author: "x",
  is_bot: false,
  body: "Body text",
  path: "src/foo.rs",
  line: 42,
  url: "https://github.com/o/r/pull/1#discussion_r1",
  replies: 0,
};

describe("commentLocation", () => {
  it("is path:line when both present", () => {
    expect(commentLocation(base)).toBe("src/foo.rs:42");
  });
  it("drops the line when absent", () => {
    expect(commentLocation({ ...base, line: null })).toBe("src/foo.rs");
  });
  it("is empty when unanchored", () => {
    expect(commentLocation({ ...base, path: null, line: null })).toBe("");
  });
});

describe("formatCommentForChat", () => {
  it("passes a bot comment through, appending only the link", () => {
    const out = formatCommentForChat({ ...base, is_bot: true });
    expect(out).toBe("Body text\n\n(https://github.com/o/r/pull/1#discussion_r1)");
  });

  it("wraps a human comment with location header and blockquote", () => {
    const out = formatCommentForChat(base);
    expect(out).toBe(
      "Address this review comment on `src/foo.rs:42`:\n" +
        "> Body text\n" +
        "(https://github.com/o/r/pull/1#discussion_r1)",
    );
  });

  it("quotes every line of a multi-line human comment", () => {
    const out = formatCommentForChat({ ...base, body: "line one\nline two" });
    expect(out).toContain("> line one\n> line two");
  });

  it("omits the location clause when the thread is unanchored", () => {
    const out = formatCommentForChat({ ...base, path: null, line: null });
    expect(out.startsWith("Address this review comment:\n")).toBe(true);
  });

  it("tolerates a missing url", () => {
    const out = formatCommentForChat({ ...base, is_bot: true, url: "" });
    expect(out).toBe("Body text");
  });
});
