import { describe, expect, it } from "vitest";
import { htmlToMarkdown, markdownImages, markdownToText } from "./markdownText";

describe("htmlToMarkdown", () => {
  it("turns a pasted screenshot into a markdown image", () => {
    const body =
      'Looks off here:\n<img width="1220" alt="Screenshot" src="https://github.com/user-attachments/assets/abc" />';
    expect(htmlToMarkdown(body)).toBe(
      "Looks off here:\n![Screenshot](https://github.com/user-attachments/assets/abc)",
    );
  });

  it("turns anchors into links and keeps bare text for anchors without an href", () => {
    expect(htmlToMarkdown('see <a href="https://x.y/z">the docs</a> or <a>this</a>')).toBe(
      "see [the docs](https://x.y/z) or this",
    );
  });

  it("promotes <summary> to a bold line and strips the wrapper tags", () => {
    const body = "<details><summary>Prompt To Fix With AI</summary>\nDo the thing.\n</details>";
    expect(htmlToMarkdown(body)).toBe("\n\n**Prompt To Fix With AI**\n\nDo the thing.\n\n");
  });

  it("strips unknown tags but keeps their text", () => {
    expect(htmlToMarkdown("<sub>tiny</sub> and <span class=x>plain</span>")).toBe("tiny and plain");
  });

  it("leaves code untouched", () => {
    const body = 'Use `<img src=x>` like:\n```html\n<a href="y">z</a>\n```';
    expect(htmlToMarkdown(body)).toBe(body);
  });

  it("does not decode entities — markdown does that", () => {
    expect(htmlToMarkdown("a &amp; b")).toBe("a &amp; b");
  });
});

describe("markdownImages", () => {
  it("lists images in order, ignoring ones inside code", () => {
    const md = "![one](https://a/1.png) text ![](https://a/2.png) `![no](https://a/3.png)`";
    expect(markdownImages(md)).toEqual([
      { alt: "one", src: "https://a/1.png" },
      { alt: "", src: "https://a/2.png" },
    ]);
  });
});

describe("markdownToText", () => {
  it("flattens a bot review into one readable line", () => {
    const md =
      "**Missing null check** on `user.profile` — see [docs](https://d).\n\n```suggestion\nif (!user) return;\n```\n\n![shot](https://a/1.png)";
    expect(markdownToText(md)).toBe("Missing null check on user.profile — see docs.");
  });

  it("drops headings, quotes and rules, and bullets lists", () => {
    expect(markdownToText("## Title\n> quoted\n- a\n- b\n---\n1. c")).toBe(
      "Title quoted • a • b • c",
    );
  });

  it("removes emphasis but not snake_case or a lone asterisk", () => {
    expect(markdownToText("*em* and __strong__ but my_var and 2 * 3")).toBe(
      "em and strong but my_var and 2 * 3",
    );
  });

  it("decodes entities for display", () => {
    expect(markdownToText("a &amp; b &lt;c&gt; &#39;d&#39; &#x41;")).toBe("a & b <c> 'd' A");
  });
});
