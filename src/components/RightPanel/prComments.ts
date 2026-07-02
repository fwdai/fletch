import type { PrComment } from "@/api";

/** Where a comment is anchored, as a `path:line` / `path` suffix — empty when
 *  the thread has no file anchor (line deleted, etc.). */
export function commentLocation(c: PrComment): string {
  if (!c.path) return "";
  return c.line != null ? `${c.path}:${c.line}` : c.path;
}

/** Build the text inserted into the chat composer by the "→ chat" action.
 *
 *  Bot reviewers (Greptile, CodeRabbit, …) already phrase their comments for
 *  an AI, so we pass the body through untouched and only append the permalink.
 *  Human comments get a short instruction + file/line context + a blockquote
 *  so the agent knows what to act on and where. */
export function formatCommentForChat(c: PrComment): string {
  const link = c.url ? `(${c.url})` : "";
  if (c.is_bot) {
    return link ? `${c.body}\n\n${link}` : c.body;
  }
  const loc = commentLocation(c);
  const header = loc
    ? `Address this review comment on \`${loc}\`:`
    : "Address this review comment:";
  const quoted = c.body
    .split("\n")
    .map((l) => `> ${l}`)
    .join("\n");
  return [header, quoted, link].filter(Boolean).join("\n");
}
