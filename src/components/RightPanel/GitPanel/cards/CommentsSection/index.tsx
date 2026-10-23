import type { PrComment, PrComments } from "@/api";
import { CommentRow } from "./CommentRow";

// ── Review comments ───────────────────────────────────────────────
// Unresolved review threads — GitHub's (Greptile / other bots / humans) and
// the local reviewer's findings — each flattened to its root comment, under
// the ladder's Review row (which carries the count). Each row previews the
// comment as plain prose, expands to the rendered markdown on demand, and
// carries its actions: hand it to the agent, drop it into the composer, open
// it on GitHub, or (a local finding) dismiss it by hand.
export function CommentsSection({
  comments,
  onAddToChat,
  onFix,
  onDismiss,
  onOpenFile,
  openable,
  localProvider,
}: {
  comments: PrComments;
  onAddToChat: (c: PrComment) => void;
  /** Hand one thread to the coding agent. Absent on a surface with no agent
   *  to hand it to. */
  onFix?: (c: PrComment) => void;
  /** Resolve a local finding by hand. Offered for local threads only. */
  onDismiss?: (c: PrComment) => void;
  /** Open a file's diff in place of the ladder; offered for the paths in
   *  `openable` (the changed files, the only ones the diff view can show). */
  onOpenFile?: (path: string) => void;
  openable?: ReadonlySet<string>;
  /** The local reviewer's base provider, for its mark. */
  localProvider?: string | null;
}) {
  const list = comments.unresolved;
  if (list.length === 0) return null;
  // The local reviewer's findings lead (highest severity first), then the
  // other bots, then humans — each already phrased for an agent to act on.
  const rows = [...list].sort(
    (a, b) =>
      Number(b.source === "local") - Number(a.source === "local") ||
      severityRank(a) - severityRank(b) ||
      Number(b.is_bot) - Number(a.is_bot),
  );
  return (
    <div className="pr-comments">
      {rows.map((c) => (
        <CommentRow
          key={c.id || c.url || `${c.author}:${c.path}:${c.line}`}
          comment={c}
          onAddToChat={onAddToChat}
          onFix={onFix}
          onDismiss={c.source === "local" ? onDismiss : undefined}
          onOpenFile={c.path && openable?.has(c.path) ? onOpenFile : undefined}
          localProvider={localProvider ?? null}
        />
      ))}
    </div>
  );
}

function severityRank(c: PrComment): number {
  switch (c.severity) {
    case "high":
      return 0;
    case "medium":
      return 1;
    case "low":
      return 2;
    default:
      return 3;
  }
}
