import { describe, expect, it } from "vitest";
import { remarkTokenChips, splitTokens, TOKEN_CHIP_ATTR, tokenPattern } from "./markdownTokens";

const pattern = (...tokens: string[]) => {
  const p = tokenPattern(new Set(tokens));
  if (!p) throw new Error("expected a pattern");
  return p;
};

describe("tokenPattern", () => {
  it("is null with nothing to match", () => {
    expect(tokenPattern(new Set())).toBeNull();
  });

  it("drops tokens `\\b` can't delimit rather than matching them loosely", () => {
    expect(tokenPattern(new Set(["-FLT-1", "FLT-1-", "#FLT"]))).toBeNull();
  });

  it("prefers the longest match — alternation is leftmost, not longest", () => {
    const p = pattern("FLT-10", "FLT-104");
    expect(splitTokens("see FLT-104", p)).toEqual([{ text: "see " }, { token: "FLT-104" }]);
  });
});

describe("splitTokens", () => {
  it("returns nothing when no token appears, so the caller keeps its node", () => {
    expect(splitTokens("nothing to see here", pattern("FLT-104"))).toEqual([]);
  });

  it("splits prose around every occurrence", () => {
    const p = pattern("FLT-104", "MCA-7");
    expect(splitTokens("FLT-104 blocks MCA-7 today", p)).toEqual([
      { token: "FLT-104" },
      { text: " blocks " },
      { token: "MCA-7" },
      { text: " today" },
    ]);
  });

  it("matches only codes on the board — a plausible shape is not enough", () => {
    expect(splitTokens("FLT-999 is not ours", pattern("FLT-104"))).toEqual([]);
  });

  it("respects word boundaries at both ends", () => {
    const p = pattern("FLT-104");
    // A longer number is a different code, and a word-glued match is not one.
    expect(splitTokens("FLT-1042 and xFLT-104", p)).toEqual([]);
    // Punctuation around it is fine — that's how a sentence quotes a code.
    expect(splitTokens("(FLT-104).", p)).toEqual([
      { text: "(" },
      { token: "FLT-104" },
      { text: ")." },
    ]);
  });

  it("is case-sensitive: the set is the authority, not the shape", () => {
    expect(splitTokens("flt-104", pattern("FLT-104"))).toEqual([]);
  });

  it("is re-runnable — a shared pattern carries no lastIndex between texts", () => {
    const p = pattern("FLT-104");
    expect(splitTokens("FLT-104", p)).toEqual([{ token: "FLT-104" }]);
    expect(splitTokens("FLT-104", p)).toEqual([{ token: "FLT-104" }]);
  });
});

// The mdast half: `remarkTokenChips` walks a real tree shape and rewrites in
// place. Literal trees rather than a parsed document, because what is under test
// is the traversal (which nodes it enters, which it refuses) and the exact node
// the renderer receives — not markdown parsing.
describe("remarkTokenChips", () => {
  /** The plugin applied to a tree, in place. `remarkTokenChips` is a plugin
   *  *factory* (built per token set so the pattern compiles once), so it is
   *  called twice: once for the set, once as unified would call it. */
  const run = (tree: unknown, ...tokens: string[]) => {
    remarkTokenChips(new Set(tokens.length ? tokens : ["FLT-104"]))()(
      tree as Parameters<ReturnType<ReturnType<typeof remarkTokenChips>>>[0],
    );
    return tree;
  };

  const text = (value: string) => ({ type: "text", value });
  /** The chip node the renderer turns into a `<button>`: an `emphasis` carrying
   *  the hast overrides, with the matched token as its only child. */
  const chip = (token: string) => ({
    type: "emphasis",
    children: [{ type: "text", value: token }],
    data: {
      hName: "button",
      hProperties: { type: "button", className: "md-chip", [TOKEN_CHIP_ATTR]: token },
    },
  });

  it("turns a token in prose into a chip node, leaving the prose around it", () => {
    const tree = {
      type: "root",
      children: [{ type: "paragraph", children: [text("see FLT-104")] }],
    };
    run(tree);
    expect(tree).toEqual({
      type: "root",
      children: [{ type: "paragraph", children: [text("see "), chip("FLT-104")] }],
    });
  });

  it("never touches code — a sample that mentions a code is a sample", () => {
    // `inlineCode` and `code` are their own node types carrying a `value` and no
    // children, so the walk has nothing to rewrite inside them.
    const tree = {
      type: "root",
      children: [
        { type: "paragraph", children: [{ type: "inlineCode", value: "FLT-104" }] },
        { type: "code", lang: "ts", value: "// FLT-104\n" },
      ],
    };
    const before = structuredClone(tree);
    run(tree);
    expect(tree).toEqual(before);
  });

  it("never touches a link — a token inside one is already a link", () => {
    const tree = {
      type: "root",
      children: [
        {
          type: "paragraph",
          children: [
            { type: "link", url: "https://x/y", children: [text("FLT-104")] },
            { type: "linkReference", identifier: "r", children: [text("FLT-104")] },
          ],
        },
      ],
    };
    const before = structuredClone(tree);
    run(tree);
    expect(tree).toEqual(before);
  });

  it("recurses through nested inline content", () => {
    // `strong` inside a `heading`: two levels below the root, and the rewrite has
    // to reach it — the PM quotes codes in headings and bold text as readily as
    // in paragraphs.
    const tree = {
      type: "root",
      children: [
        {
          type: "heading",
          depth: 2,
          children: [{ type: "strong", children: [text("FLT-104 first")] }],
        },
      ],
    };
    run(tree);
    expect(tree.children[0].children[0]).toEqual({
      type: "strong",
      children: [chip("FLT-104"), text(" first")],
    });
  });

  it("does nothing at all with no tokens to match", () => {
    // No pattern to compile, so the tree is never walked — the case every chat
    // outside the roadmap is in.
    const tree = { type: "root", children: [{ type: "paragraph", children: [text("FLT-104")] }] };
    const before = structuredClone(tree);
    remarkTokenChips(new Set())()(tree);
    expect(tree).toEqual(before);
  });
});
