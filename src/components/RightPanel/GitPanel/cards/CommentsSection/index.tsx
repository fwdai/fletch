import type { PrComment, PrComments } from "@/api";
import { CommentRow } from "./CommentRow";

// ── Review comments ───────────────────────────────────────────────
// Unresolved PR review threads (Greptile / other bots / humans), each
// flattened to its root comment. Mirrors ChecksSection's visual language.
// Each row previews the comment as plain prose, expands to the rendered
// markdown on demand, and carries two always-visible actions: "Add to chat"
// (drops the comment into the composer for the user to send to the agent)
// and "GitHub" (opens the thread).
export function CommentsSection({
  comments,
  onAddToChat,
}: {
  comments: PrComments;
  onAddToChat: (c: PrComment) => void;
}) {
  const list = comments.unresolved;
  if (list.length === 0) return null;
  // Bots (the AI reviewers this feature targets) lead; otherwise stable order.
  const rows = [...list].sort((a, b) => Number(b.is_bot) - Number(a.is_bot));
  return (
    <div className="pr-comments">
      <div className="pr-comments-h text-xs">
        <span>Comments</span>
        <span className="pr-comments-sum">{list.length} unresolved</span>
      </div>
      {rows.map((c) => (
        <CommentRow key={c.url} comment={c} onAddToChat={onAddToChat} />
      ))}
    </div>
  );
}
