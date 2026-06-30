import { open } from "@tauri-apps/plugin-shell";
import type { PrComment, PrComments } from "@/api";
import { Icon } from "@/components/Icon";
import { commentLocation } from "@/components/RightPanel/prComments";

// ── Review comments ───────────────────────────────────────────────
// Unresolved PR review threads (Greptile / other bots / humans), each
// flattened to its root comment. Mirrors ChecksSection's visual language.
// Each row links out to the thread (↗) and offers a "→ chat" quick action
// that drops the comment into the composer for the user to send to the agent.
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
      {rows.map((c) => {
        const loc = commentLocation(c);
        return (
          <div key={c.url} className="pr-comment">
            <Icon name={c.is_bot ? "bot" : "user"} size={12} />
            <div className="pc-body">
              <div className="pc-top text-xs">
                <span className="pc-author">{c.author}</span>
                {loc && <span className="pc-loc text-xs">{loc}</span>}
                {c.replies > 0 && (
                  <span className="pc-replies">
                    +{c.replies} {c.replies === 1 ? "reply" : "replies"}
                  </span>
                )}
              </div>
              <div className="pc-text text-sm">{c.body}</div>
            </div>
            <div className="pc-acts">
              <button
                type="button"
                className="pc-act tip"
                data-tip="Add to chat"
                aria-label="Add comment to chat"
                onClick={() => onAddToChat(c)}
              >
                <Icon name="arrowR" size={12} />
              </button>
              <button
                type="button"
                className="pc-act tip"
                data-tip="View on GitHub"
                aria-label="View comment on GitHub"
                onClick={() => void open(c.url)}
              >
                <Icon name="external" size={11} />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
