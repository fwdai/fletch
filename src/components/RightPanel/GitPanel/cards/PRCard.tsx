import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks, PrComment, PrComments, PrState } from "../../../../api";
import { Icon } from "../../../Icon";
import { describeMergeGate, type MergeGateSituation } from "../../mergeGate";
import { ChecksSection } from "./ChecksSection";
import { CommentsSection } from "./CommentsSection";

/** The card's verbose merge-gate line. `blocked` collapses its two sub-states
 *  (failing checks vs. review gate) into one message here. */
const CARD_GATE_BY_SITUATION: Record<
  MergeGateSituation,
  { cls: string; text: (base: string) => string }
> = {
  ready: { cls: "ok", text: () => "✓ Ready to merge" },
  "mergeable-soft": { cls: "ok", text: () => "✓ Mergeable — optional checks failing" },
  "checks-failing": { cls: "att", text: () => "△ Blocked by required checks or reviews" },
  "review-required": { cls: "att", text: () => "△ Blocked by required checks or reviews" },
  behind: { cls: "att", text: (base) => `△ Behind ${base} — update your branch` },
  conflicts: { cls: "att", text: (base) => `△ Conflicts with ${base} — update your branch` },
  draft: { cls: "ok", text: () => "Draft — mark ready on GitHub to merge" },
  computing: { cls: "ok", text: () => "Computing merge status…" },
  "no-conflicts": { cls: "ok", text: () => "✓ No merge conflicts" },
  "cant-merge": {
    cls: "att",
    text: (base) => `△ Can’t merge cleanly with ${base} — update your branch`,
  },
};

export function PRCard({
  pr,
  base,
  checks,
  comments,
  onAddToChat,
}: {
  pr: PrState;
  base: string;
  checks: PrChecks | null;
  comments: PrComments | null;
  onAddToChat: (c: PrComment) => void;
}) {
  // One merge-gate line. Gate semantics live in describeMergeGate (spec §6);
  // the card just renders the verbose copy for the resulting situation.
  const { situation } = describeMergeGate(checks?.merge_state ?? null, {
    checksFailed: checks?.failed ?? 0,
    mergeable: pr.mergeable,
  });
  const gate = CARD_GATE_BY_SITUATION[situation];
  return (
    <div className="git-card">
      <div className="git-card-h">Pull request</div>
      <div className="git-card-title">{pr.title}</div>
      <div className="git-card-meta">#{pr.number} · open</div>
      <div className="git-card-row">
        <span className={gate.cls}>{gate.text(base)}</span>
      </div>
      {checks && <ChecksSection checks={checks} prUrl={pr.url} />}
      {comments && <CommentsSection comments={comments} onAddToChat={onAddToChat} />}
      <div className="git-card-links">
        <button type="button" className="git-card-link" onClick={() => void open(pr.url)}>
          <Icon name="github" size={11} />
          Overview
        </button>
        <button
          type="button"
          className="git-card-link"
          onClick={() => void open(`${pr.url}/files`)}
        >
          <Icon name="diff" size={11} />
          Files
        </button>
        <button
          type="button"
          className="git-card-link"
          onClick={() => void open(`${pr.url}/commits`)}
        >
          <Icon name="commit" size={11} />
          Commits
        </button>
      </div>
    </div>
  );
}

export function ClosedPRCard({ pr }: { pr: PrState }) {
  return (
    <div className="git-card">
      <div className="git-card-h">Pull request</div>
      <div className="git-card-title">{pr.title}</div>
      <div className="git-card-meta">#{pr.number} · closed</div>
      <button type="button" className="git-card-link" onClick={() => void open(pr.url)}>
        <Icon name="github" size={11} />
        View on GitHub
      </button>
    </div>
  );
}
