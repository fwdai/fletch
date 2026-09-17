import type { PrComment } from "@/api";
import { htmlToMarkdown } from "@/util/markdownText";

/** Where a comment is anchored, as a `path:line` / `path` suffix — empty when
 *  the thread has no file anchor (line deleted, etc.). */
export function commentLocation(c: PrComment): string {
  if (!c.path) return "";
  return c.line != null ? `${c.path}:${c.line}` : c.path;
}

/** Build the text inserted into the chat composer by the "→ chat" action.
 *
 *  The body's inline HTML (pasted screenshots, bot `<details>` wrappers, raw
 *  anchors) is rewritten to markdown first, so the composer gets readable text
 *  with images as `![alt](url)` links the agent can fetch, not tag soup.
 *
 *  Bot reviewers (Greptile, CodeRabbit, …) already phrase their comments for
 *  an AI, so we pass that body through as-is and only append the permalink.
 *  Human comments get a short instruction + file/line context + a blockquote
 *  so the agent knows what to act on and where. */
export function formatCommentForChat(c: PrComment): string {
  const body = htmlToMarkdown(c.body).trim();
  const link = c.url ? `(${c.url})` : "";
  if (c.is_bot) {
    return link ? `${body}\n\n${link}` : body;
  }
  const loc = commentLocation(c);
  const header = loc
    ? `Address this review comment on \`${loc}\`:`
    : "Address this review comment:";
  const quoted = body
    .split("\n")
    .map((l) => `> ${l}`)
    .join("\n");
  return [header, quoted, link].filter(Boolean).join("\n");
}
