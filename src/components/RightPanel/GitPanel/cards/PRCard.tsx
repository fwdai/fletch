import { open } from "@tauri-apps/plugin-shell";
import type { PrChecks, PrComment, PrComments, PrState } from "@/api";
import { Icon } from "@/components/Icon";
import { describeMergeGate, type MergeGateSituation } from "@/mergeGate";
import type { PrOrigin } from "../hooks/usePrOrigin";
import { ChecksSection } from "./ChecksSection";
import { CommentsSection } from "./CommentsSection";

/** The card's verbose merge-gate line. Failing checks and a review gate get
 *  their own message: `checks-failing` no longer implies the gate is shut (an
 *  `unstable` PR is both red and mergeable), so one collapsed "blocked by checks
 *  or reviews" line would misstate both halves. */
const CARD_GATE_BY_SITUATION: Record<
  MergeGateSituation,
  { cls: string; text: (base: string) => string }
> = {
  ready: { cls: "ok", text: () => "✓ Ready to merge" },
  "mergeable-soft": { cls: "ok", text: () => "✓ Mergeable — checks still running" },
  "checks-failing": { cls: "att", text: () => "△ Checks failing" },
  "review-required": { cls: "att", text: () => "△ Blocked by a review gate" },
  behind: { cls: "att", text: (base) => `△ Behind ${base} — update your branch` },
  conflicts: { cls: "att", text: (base) => `△ Conflicts with ${base} — update your branch` },
  draft: { cls: "ok", text: () => "Draft — mark ready on GitHub to merge" },
  computing: { cls: "ok", text: () => "Computing merge status…" },
  "no-conflicts": { cls: "ok", text: () => "✓ No merge conflicts" },
};

export function PRCard({
  pr,
  branch,
  origin,
  base,
  checks,
  comments,
  onAddToChat,
}: {
  pr: PrState;
  /** The PR's head branch, as a provenance line under the meta. The caller
   *  passes it only when the checkout is on another branch (the focused PR is
   *  not necessarily the one checked out), so the usual card stays as it was. */
  branch?: string | null;
  /** The sub-agent that opened this PR, when one did — said on the branch
   *  line, one click from its thread. */
  origin?: PrOrigin | null;
  base: string;
  checks: PrChecks | null;
  comments: PrComments | null;
  onAddToChat: (c: PrComment) => void;
}) {
  // One merge-gate line. Gate semantics live in describeMergeGate (spec §6);
  // the card just renders the verbose copy for the resulting situation.
  const { situation } = describeMergeGate(checks?.merge_state ?? null, {
    // `required_failing` (the named failing runs), not `failed` — the latter
    // double-counts a rerun. `readiness.ts` owns that definition and every other
    // caller now derives it the same way.
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: pr.mergeable,
  });
  const gate = CARD_GATE_BY_SITUATION[situation];
  return (
    <div className="git-card">
      <div className="git-card-h text-xs">Pull request</div>
      <div className="git-card-title text-base">{pr.title}</div>
      <div className="git-card-meta text-sm">#{pr.number} · open</div>
      {(branch || origin) && (
        <div className="git-card-branch text-xs flex-center" title={branch ?? undefined}>
          <Icon name={branch ? "branch" : "subagent"} size={11} />
          {branch && <span className="git-card-branch-name">{branch}</span>}
          {origin && (
            <span className="git-card-origin">
              {branch ? "· " : ""}opened by{" "}
              <button type="button" className="git-card-origin-link" onClick={origin.open}>
                {origin.label}
              </button>
            </span>
          )}
        </div>
      )}
      <div className="git-card-row text-sm">
        <span className={gate.cls}>{gate.text(base)}</span>
      </div>
      {checks && <ChecksSection checks={checks} prUrl={pr.url} />}
      {comments && <CommentsSection comments={comments} onAddToChat={onAddToChat} />}
      <div className="git-card-links">
        <button
          type="button"
          className="git-card-link iflex-center"
          onClick={() => void open(pr.url)}
        >
          <Icon name="github" size={11} />
          Overview
        </button>
        <button
          type="button"
          className="git-card-link iflex-center"
          onClick={() => void open(`${pr.url}/files`)}
        >
          <Icon name="diff" size={11} />
          Files
        </button>
        <button
          type="button"
          className="git-card-link iflex-center"
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
      <div className="git-card-h text-xs">Pull request</div>
      <div className="git-card-title text-base">{pr.title}</div>
      <div className="git-card-meta text-sm">#{pr.number} · closed</div>
      <button
        type="button"
        className="git-card-link iflex-center"
        onClick={() => void open(pr.url)}
      >
        <Icon name="github" size={11} />
        View on GitHub
      </button>
    </div>
  );
}
