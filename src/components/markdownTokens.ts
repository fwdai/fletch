// Turning bare tokens in markdown prose into clickable chips.
//
// One surface needs this today: the roadmap's PM chat, where an item code the PM
// quotes ("FLT-104") should jump the board beside it to that row. That chat
// renders through the shared `Markdown` component, and the codes are known only
// to the roadmap — so the set arrives by context (`TokenChipContext`) and the
// rewrite happens in the mdast rather than in rendered DOM: `code`/`inlineCode`
// are their own node types and link subtrees are skipped here, so a token inside
// a code sample or an existing link is never touched.
//
// The tokens build the pattern (escaped, longest first), so nothing here guesses
// a token's *shape*. Exact set membership is the only rule — which is what keeps
// one project's code prefix from lighting up in another project's chat, and what
// keeps a plausible-looking string that isn't on the board from pretending to be.

/** Attribute a rendered chip carries. The host surface handles clicks by
 *  delegation on its own container, so no callback is threaded through the
 *  shared renderer (and a chip stays a real `<button>`, keyboard included). */
export const TOKEN_CHIP_ATTR = "data-token-chip";

/** One run of a text node: prose, or a token that should become a chip. */
export type TokenSegment = { text: string } | { token: string };

/** Escape a token for use inside a regex alternation. */
function escaped(token: string): string {
  return token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** One pattern matching any of `tokens` as a whole word, or null when there is
 *  nothing to match.
 *
 *  Longest first, so `FLT-10` can't win a match that `FLT-104` should have had —
 *  regex alternation is leftmost-first, not longest.
 *
 *  Tokens that don't begin and end with a word character are dropped: `\b` can't
 *  delimit them, and a token that matches unpredictably is worse than one that
 *  doesn't match at all. */
export function tokenPattern(tokens: ReadonlySet<string>): RegExp | null {
  const list = [...tokens].filter((t) => /^\w[\w-]*\w$/.test(t));
  if (list.length === 0) return null;
  list.sort((a, b) => b.length - a.length);
  return new RegExp(`\\b(?:${list.map(escaped).join("|")})\\b`, "g");
}

/** Split one text run into prose and token segments.
 *
 *  Returns an empty array when nothing matched — the caller then keeps its
 *  original node instead of rebuilding an identical one. */
export function splitTokens(text: string, pattern: RegExp): TokenSegment[] {
  const out: TokenSegment[] = [];
  let last = 0;
  pattern.lastIndex = 0;
  for (let m = pattern.exec(text); m != null; m = pattern.exec(text)) {
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ token: m[0] });
    last = m.index + m[0].length;
  }
  if (out.length === 0) return [];
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** The slice of mdast this rewrite touches. Deliberately structural rather than
 *  imported from `mdast`: the transform only reads `type`/`value`/`children` and
 *  writes `data`, and staying loose keeps it testable without a parser. */
interface MdNode {
  type: string;
  value?: string;
  children?: MdNode[];
  data?: Record<string, unknown>;
}

/** A token as a node the renderer turns into a `<button>`.
 *
 *  An `emphasis` node carrying `data.hName`/`hProperties`, which
 *  mdast-util-to-hast applies over whatever the node's own handler produced. Two
 *  reasons for that base type: it is ordinary inline content every mdast
 *  consumer already understands, and its handler contributes no attributes of
 *  its own — so the chip's properties are exactly these. (A `link` node would
 *  have been the obvious choice and is the wrong one: it forces a `url`, which
 *  arrives as an `href` the renderer then has to sanitize, on a button.) */
function chip(token: string): MdNode {
  return {
    type: "emphasis",
    children: [{ type: "text", value: token }],
    data: {
      hName: "button",
      hProperties: { type: "button", className: "md-chip", [TOKEN_CHIP_ATTR]: token },
    },
  };
}

/** Rewrite one node's children in place. Recursive rather than a visitor
 *  dependency: the tree is small and the two skip rules (links, and anything
 *  without children) are the whole traversal. */
function rewrite(node: MdNode, pattern: RegExp): void {
  if (!node.children) return;
  // A token inside a link is already a link; rewriting it would nest one.
  if (node.type === "link" || node.type === "linkReference") return;
  const next: MdNode[] = [];
  let changed = false;
  for (const child of node.children) {
    if (child.type === "text" && typeof child.value === "string") {
      const segments = splitTokens(child.value, pattern);
      if (segments.length === 0) {
        next.push(child);
        continue;
      }
      changed = true;
      for (const s of segments) {
        next.push("token" in s ? chip(s.token) : { type: "text", value: s.text });
      }
      continue;
    }
    rewrite(child, pattern);
    next.push(child);
  }
  if (changed) node.children = next;
}

/** A remark plugin turning every exact `tokens` match in prose into a chip.
 *  Built per token set, so the pattern is compiled once rather than per node. */
export function remarkTokenChips(tokens: ReadonlySet<string>) {
  const pattern = tokenPattern(tokens);
  return () => (tree: MdNode) => {
    if (pattern) rewrite(tree, pattern);
  };
}
