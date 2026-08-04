import { issueDisplayKey } from "@/api";
import { Icon } from "@/components/Icon";
import type { FunnelAction } from "./funnel";
import { IssueRoadmapAction } from "./IssueRoadmapAction";
import type { InboxRow } from "./inbox";

/** Compact "updated N ago" hint from a ms-epoch, or "" when unknown. Mirrors
 *  History's formatter so the whole app speaks the same relative time. */
function updatedAgo(ms: number | undefined): string {
  if (!ms) return "";
  const minutes = Math.floor((Date.now() - ms) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** One inbox row: issue number + title, quiet label chips, and the two things
 *  it can become — a working agent now ("Start work", prefilled composer), or a
 *  ghost row on the project's roadmap to rule on later. The row is compact and
 *  calm — secondary to the review queue above it. */
export function IssueRow({
  row,
  showRepo,
  onStart,
  funnel,
  onAddToRoadmap,
}: {
  row: InboxRow;
  /** Show the originating repo (only meaningful when >1 repo has issues). */
  showRepo: boolean;
  onStart: () => void;
  funnel: FunnelAction;
  onAddToRoadmap: () => void;
}) {
  const { issue } = row;
  const ago = updatedAgo(issue.updated_at);
  return (
    <div className="mc-inbox-row">
      <div className="mc-inbox-body">
        <div className="mc-inbox-titleline">
          <span className="mc-inbox-num">{issueDisplayKey(issue)}</span>
          <span className="mc-inbox-title" title={issue.title}>
            {issue.title}
          </span>
          <a
            className="mc-inbox-ext"
            href={issue.url}
            target="_blank"
            rel="noreferrer"
            title={issue.source === "linear" ? "Open in Linear" : "Open on GitHub"}
            onClick={(e) => e.stopPropagation()}
          >
            <Icon name="external" size={12} />
          </a>
        </div>
        <div className="mc-inbox-meta">
          {showRepo && <span className="mc-inbox-repo">{row.repoLabel}</span>}
          {issue.labels.slice(0, 4).map((l) => (
            <span className="mc-issue-label" key={l.name}>
              {l.color && (
                <span className="mc-issue-dot" style={{ backgroundColor: `#${l.color}` }} />
              )}
              {l.name}
            </span>
          ))}
          {issue.assignee && <span className="mc-inbox-hint">@{issue.assignee}</span>}
          {ago && <span className="mc-inbox-hint">{ago}</span>}
        </div>
      </div>
      <div className="mc-inbox-actions">
        <IssueRoadmapAction action={funnel} onAdd={onAddToRoadmap} />
        <button type="button" className="mc-btn mc-inbox-start" onClick={onStart}>
          <Icon name="play" size={12} /> Start work
        </button>
      </div>
    </div>
  );
}
