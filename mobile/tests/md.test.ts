// The chat's markdown renderer, rendered to static HTML. What matters is that
// the syntax an agent actually emits becomes elements rather than literal
// characters — the hand-rolled renderer this replaced printed "## " and single
// asterisks verbatim.
//
// `createElement` rather than JSX because the suite's glob is `tests/**/*.test.ts`.
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Md } from "../src/components/Md";

const html = (text: string) => renderToStaticMarkup(createElement(Md, { text }));

describe("Md", () => {
  it("renders headings", () => {
    expect(html("## Section")).toBe("<h2>Section</h2>");
  });

  it("renders emphasis and strong", () => {
    expect(html("*hello* and **bold**")).toBe("<p><em>hello</em> and <strong>bold</strong></p>");
  });

  it("renders both list flavours", () => {
    expect(html("- one\n- two")).toContain("<ul>");
    expect(html("1. one\n2. two")).toContain("<ol>");
  });

  it("renders inline and fenced code", () => {
    expect(html("use `bun run check`")).toContain("<code>bun run check</code>");
    expect(html("```ts\nconst a = 1;\n```")).toContain("<pre><code");
  });

  it("opens links outside the webview", () => {
    const out = html("[docs](https://example.com)");
    expect(out).toContain('href="https://example.com"');
    expect(out).toContain('target="_blank"');
    expect(out).toContain('rel="noreferrer"');
  });

  it("renders gfm tables and strikethrough", () => {
    expect(html("| a | b |\n| --- | --- |\n| 1 | 2 |")).toContain("<table>");
    expect(html("~~gone~~")).toContain("<del>gone</del>");
  });
});
