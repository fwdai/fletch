// Readable views of GitHub-flavoured markdown that may carry inline HTML.
//
// GitHub comment bodies mix markdown with a handful of HTML tags: pasted
// screenshots arrive as `<img src=… alt=…>`, bot reviewers wrap sections in
// `<details><summary>` and link out with `<a href>`. We never render that HTML
// as HTML — `htmlToMarkdown` rewrites the tags we care about into their
// markdown equivalents and strips the rest, so the shared Markdown renderer
// shows links as links and images as images without a raw-HTML plugin (and
// without any sanitisation surface). `markdownToText` flattens the result to
// one line of prose for compact previews.
//
// Code is left alone throughout: a `<img>` inside a fenced block or a code
// span is code, not markup.

/** Fenced blocks and inline code spans — the stretches transforms must skip. */
const CODE_SPANS = /(```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]*`)/g;

/** Apply `fn` to every stretch of `md` that is not code. */
function outsideCode(md: string, fn: (prose: string) => string): string {
  return md
    .split(CODE_SPANS)
    .map((part, i) => (i % 2 === 1 ? part : fn(part)))
    .join("");
}

function attr(tag: string, name: string): string | undefined {
  const m = tag.match(new RegExp(`\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, "i"));
  return m ? (m[1] ?? m[2] ?? m[3]) : undefined;
}

const stripTags = (s: string) => s.replace(/<\/?[a-zA-Z][^>]*>/g, "");

/** Rewrite the HTML GitHub comments actually contain into markdown; drop any
 *  other tag while keeping its text. Entities are left encoded — markdown
 *  decodes them itself. */
export function htmlToMarkdown(body: string): string {
  return outsideCode(body, (s) =>
    s
      .replace(/<br\s*\/?>/gi, "\n")
      .replace(/<\/?(p|div|details|blockquote|ul|ol|li|h[1-6])\b[^>]*>/gi, "\n\n")
      .replace(/<summary\b[^>]*>([\s\S]*?)<\/summary>/gi, (_m, t: string) => `**${t.trim()}**\n\n`)
      .replace(/<img\b[^>]*>/gi, (tag) => {
        const src = attr(tag, "src");
        if (!src) return "";
        return `![${attr(tag, "alt") ?? "image"}](${src})`;
      })
      .replace(/<a\b([^>]*)>([\s\S]*?)<\/a>/gi, (_m, attrs: string, inner: string) => {
        const href = attr(attrs, "href");
        const text = stripTags(inner).trim();
        if (!href) return text;
        return text ? `[${text}](${href})` : href;
      })
      .replace(/<\/?[a-zA-Z][^>]*>/g, "")
      .replace(/\n{3,}/g, "\n\n"),
  );
}

const ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
};

function decodeEntities(s: string): string {
  return s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (m, e: string) => {
    if (e[0] === "#") {
      const code =
        e[1] === "x" || e[1] === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : m;
    }
    return ENTITIES[e.toLowerCase()] ?? m;
  });
}

/** Every image in `md` (after `htmlToMarkdown`), in order. */
export function markdownImages(md: string): { src: string; alt: string }[] {
  const out: { src: string; alt: string }[] = [];
  outsideCode(md, (s) => {
    for (const m of s.matchAll(/!\[([^\]]*)\]\(\s*(\S+?)(?:\s+"[^"]*")?\s*\)/g)) {
      out.push({ alt: m[1], src: m[2] });
    }
    return s;
  });
  return out;
}

/** Flatten markdown to a single line of readable prose for a preview: code
 *  blocks and images are dropped, links keep their text, emphasis and
 *  structure markers go, whitespace collapses. */
export function markdownToText(md: string): string {
  const text = md
    .replace(/```[\s\S]*?```|~~~[\s\S]*?~~~/g, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/<(https?:\/\/[^>\s]+)>/g, "$1")
    .replace(/`([^`\n]*)`/g, "$1")
    .replace(/^[ \t]*(?:#{1,6}|>)+[ \t]*/gm, "")
    .replace(/^[ \t]*(?:[-*+]|\d+[.)])[ \t]+/gm, "• ")
    .replace(/^[ \t]*(?:-{3,}|\*{3,}|_{3,})[ \t]*$/gm, " ")
    .replace(/(\*\*|__|~~)(.+?)\1/g, "$2")
    .replace(/(^|[^*\w])\*([^*\n]+)\*(?![*\w])/g, "$1$2");
  return decodeEntities(text).replace(/\s+/g, " ").trim();
}
