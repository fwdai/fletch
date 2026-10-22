import { useEffect, useMemo, useState } from "react";
import type { AgentRecord, PrComments, ReviewPhase, TrackedRepo } from "@/api";
import { isCommitAction } from "@/components/RightPanel/primaryActions";
import { providerLabel } from "@/data/providers";
import { delegationLabel } from "@/delegation";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { checkoutKey } from "@/store/git";
import { resolveReviewer } from "@/store/review";
import { ActionBar } from "./ActionBar";
import { AutopilotHistory } from "./Autopilot";
import { CommitComposer, CommitStatus } from "./CommitComposer";
import { BlockedConfigCard } from "./cards";
import { useCommitDraft } from "./hooks/useCommitDraft";
import { useFooterModel } from "./hooks/useFooterModel";
import { useGitActions } from "./hooks/useGitActions";
import { useGitPanelData } from "./hooks/useGitPanelData";
import { usePrOrigin } from "./hooks/usePrOrigin";
import { useTransientFeedback } from "./hooks/useTransientFeedback";
import type { LadderBodies, ReviewBodies } from "./Ladder";
import { LaneBody } from "./LaneBody";
import { buildLadder } from "./ladderModel";
import type { Lane } from "./laneModel";
import { WorktreesNote } from "./WorktreesNote";

/** The clock the Review row reads: a second while a review runs (its timer),
 *  a minute otherwise (the age of a verdict). */
function useReviewClock(phase: ReviewPhase | undefined): number {
  const [now, setNow] = useState(() => Date.now());
  const fast = phase === "reviewing";
  useEffect(() => {
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), fast ? 1000 : 60_000);
    return () => clearInterval(id);
  }, [fast]);
  return now;
}

/** The local reviewer's threads alone — what a lane with no PR shows, since
 *  GitHub's threads belong to the focused PR's lane. Null when there are none. */
function localThreads(comments: PrComments | null): PrComments | null {
  const local = comments?.unresolved.filter((t) => t.source === "local") ?? [];
  return local.length > 0 ? { unresolved: local } : null;
}

const NO_REVIEWER = "Pick a reviewer in Settings › Review";

/** The git panel: lanes (header) · ladder (body) · action bar (footer). The
 *  lanes span the workspace (`GitPanel` builds them); the ladder and the footer
 *  describe and act on the checkout the viewed lane belongs to — `repo` and
 *  `subdir`, undefined for the agent's primary repo (index 0), which reads and
 *  writes the plain agent-keyed store entries that live events and bulk polls
 *  update; secondaries read/write under `checkoutKey(agentId, subdir)`.
 *
 *  The component is a thin orchestrator: live reads + polling live in
 *  `useGitPanelData`, the commit draft in `useCommitDraft`, busy/notice in
 *  `useTransientFeedback`, the dispatch table in `useGitActions`, the footer
 *  in `useFooterModel`, and the projections the body draws in `laneModel.ts` /
 *  `ladderModel.ts`. The agent-handoff lifecycle is NOT here — it belongs to
 *  the host (`supervisor::delegation`), so a delegation completes whether or
 *  not this section, or any window, is on screen; the section only reads the
 *  store's mirror of it to render it. */
export function GitRepoSection({
  agent,
  repo,
  subdir,
  viewed,
}: {
  agent: AgentRecord;
  repo: TrackedRepo | undefined;
  subdir?: string;
  /** The lane the ladder describes, from this checkout. */
  viewed: Lane | null;
}) {
  const { gitState, prState, checks, comments, mergeState, fetchGitState, fetchPrState } =
    useGitPanelData(agent.id, repo, subdir);

  const key = checkoutKey(agent.id, subdir);
  const { busy, runBusy, notice, showNotice } = useTransientFeedback(key);
  // Something to commit: uncommitted files, none of them conflicted (a
  // conflict is the agent's to reconcile first, not a message to write).
  const committable =
    (gitState?.files.length ?? 0) > 0 && !gitState?.files.some((f) => f.kind === "conflicted");
  // A half-written message is this checkout's: a multi-repo workspace drops
  // it when the lane in view moves to another repo, never carrying it across.
  const { override, msg, setMsg, commitRef, customActive, openOverride, revertOverride } =
    useCommitDraft(key, committable);

  // This checkout's in-flight delegation, for display only — the lifecycle that
  // clears it runs on the host. Its settled outcome arrives the same way: the
  // host's `done` / `abandoned` event posts it to the store, and we merge it
  // with this section's own action notices.
  const delegation = useAppStore((s) => s.delegations[key]);
  const delegationNotice = useAppStore((s) => s.delegationNotices[key]);
  // Autopilot is how the agent behaves on the focused PR (switched per project
  // in settings, pausable per workspace from the lanes). The action bar's
  // status slot carries the one thing worth knowing about it: that the
  // in-flight turn was started automatically. What it did afterwards is the
  // body's history.
  const autopilot = useAppStore((s) => s.autopilot[key]);
  const autoAttempt = autopilot?.cycle?.phase === "working" ? autopilot.cycle.attempt : null;

  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  const hasOrigin = gitState?.has_origin ?? true;

  // The local reviewer on this checkout: the host's state (a mirror, like
  // `delegations`), the custom agent that would review, and why a review
  // can't start — an older host, or no reviewer picked.
  const review = useAppStore((s) => s.reviews[key] ?? null);
  const customAgents = useAppStore((s) => s.customAgents);
  const reviewAgentId = useAppStore((s) => s.reviewAgentId);
  const reviewer = useMemo(
    () => resolveReviewer(customAgents, reviewAgentId),
    [customAgents, reviewAgentId],
  );
  const reviewGate = useGate("review");
  const reviewRefusal = reviewGate ?? (reviewer ? null : NO_REVIEWER);
  const nowMs = useReviewClock(review?.phase);
  // Who a queued review waits for, as the composer names the agent: its
  // custom agent, else its provider.
  const coderName =
    customAgents.find((a) => a.id === agent.custom_agent_id)?.name ?? providerLabel(agent.provider);

  const branch = gitState?.branch || repo?.branch || "";
  const base = gitState?.parent_branch || repo?.parent_branch || "main";
  // The checkout is detached until its first push; a branch is only born from
  // an agent that names it. So a direct (agent-bypassed) action that needs a
  // branch — push, open PR — can't run yet: it routes through the agent
  // instead, which picks a conventional name and creates the branch.
  const hasBranch = Boolean(branch);

  // ── The lane in view ───────────────────────────────────────────────────────
  // The lane the footer's actions apply to: the focused PR's lane, or the
  // tree's own lane when no PR covers its branch. Any other lane can only be
  // looked at — and focused.
  const acting = viewed == null || viewed.pr == null || viewed.focused;
  // What the ladder AND the footer describe, so they can never disagree: the
  // focused PR's live state on its own lane; nothing on the tree's bare lane
  // while another PR is focused (a commit from there lands on its branch, a
  // push from there wants its own proposal — not that PR's merge).
  const laneHasPr = viewed?.pr != null;
  const lanePr = laneHasPr ? prState : null;
  const laneChecks = laneHasPr ? checks : null;
  const laneMergeState = laneHasPr ? mergeState : null;
  // The tree's own lane still shows the local reviewer's findings: they are
  // about the checkout, not about any PR.
  const laneComments = laneHasPr ? comments : localThreads(comments);
  // The PR in view with everything the store knows about it: the focused PR's
  // live state when it is the one in view, else the set entry's snapshot.
  const viewedPr = viewed?.pr ? (acting ? prState : viewed.pr.state) : null;
  const viewedChecks = viewed?.pr ? (acting ? checks : viewed.pr.checks) : null;

  // The footer: the ladder's next step as the primary, the rest as a menu —
  // or, for a lane the panel only looks at, the way to focus it. Every action
  // carries the one gate; the dispatch below re-runs it.
  const { primary, groups, tone, mainDisabled, mainReason, ending, gate } = useFooterModel({
    mode: acting ? "acting" : "focus",
    gitState,
    prState: acting ? lanePr : (viewedPr ?? null),
    checks: laneChecks,
    mergeState: laneMergeState,
    comments: laneComments,
    base,
    customActive,
    delegationActive: delegation != null,
    busy: busy != null,
    githubConnected,
    primaryCheckout: subdir === undefined,
    reviewMenu: acting
      ? { state: review, reviewerName: reviewer?.name ?? null, refusal: reviewRefusal }
      : undefined,
  });

  const { runAction, addCommentToChat, fixThread, dismissThread } = useGitActions({
    agentId: agent.id,
    subdir,
    base,
    hasBranch,
    customActive,
    msg,
    checks: laneChecks,
    comments: laneComments,
    prUrl: viewedPr?.url,
    prNumber: viewedPr?.number,
    gate,
    githubConnected,
    hasOrigin,
    runBusy,
    showNotice,
    openOverride,
    revertOverride,
    fetchPrState,
    review,
    projectId: agent.project_id,
  });
  const setCommitMode = useAppStore((s) => s.setGitCommitAction);
  // A menu row: a commit mode becomes the sticky default (and so, while there
  // is something to commit, the primary); anything else runs.
  const onPick = (k: string) => {
    if (isCommitAction(k)) setCommitMode(k);
    else runAction(k);
  };

  // The same five rows for every lane. A lane only looked at — another PR of
  // the checkout — is drawn from its set entry, with the tree's rows and the
  // review row idle and saying why.
  const rows = useMemo(() => {
    if (!viewed) return [];
    if (!acting && viewed.pr) {
      return buildLadder({
        git: gitState,
        pr: viewed.pr.state,
        checks: viewed.pr.checks,
        mergeState: viewed.pr.checks?.merge_state ?? null,
        comments: null,
        base,
        lane: { tree: false, focused: false },
      });
    }
    return buildLadder({
      git: gitState,
      pr: lanePr,
      checks: laneChecks,
      mergeState: laneMergeState,
      comments: laneComments,
      base,
      review,
      canReview: reviewGate == null,
      nowMs,
      coderName,
    });
  }, [
    coderName,
    viewed,
    acting,
    gitState,
    lanePr,
    laneChecks,
    laneMergeState,
    laneComments,
    base,
    review,
    reviewGate,
    nowMs,
  ]);

  // The Review row's controls: one button (start / re-run / stop), and the
  // per-thread fix and dismiss. On a lane the footer doesn't act on, none.
  // Stop also cancels a start the host is holding for the coder's turn.
  const reviewing = review?.phase === "reviewing" || review?.phase === "queued";
  const reviewAgain = review != null && review.phase !== "idle" && review.phase !== "failed";
  const reviewBodies: ReviewBodies | null =
    acting && !reviewGate
      ? {
          state: review,
          action: {
            label: reviewing ? "Stop" : reviewAgain ? "Re-review" : "Review",
            reason: reviewing ? null : reviewRefusal,
            onRun: () => runAction(reviewing ? "review-stop" : "review-start"),
          },
          onFix: fixThread,
          onDismiss: dismissThread,
          onOpenReviewer: review?.reviewer?.agent_id ? () => runAction("review-open") : null,
        }
      : null;

  // ── Ladder bodies ──────────────────────────────────────────────────────────
  // Pushed commits link out to GitHub — a single commit when only one is ahead,
  // otherwise the base..branch compare (commit list + full diff). Gated on
  // nothing being unpushed, so the tip is on origin and the link can't 404.
  // Needs the origin web base (github.com remotes only).
  const webBase = gitState?.remote_url ?? null;
  const aheadCount = gitState?.ahead ?? 0;
  const unpushed = gitState?.unpushed ?? 0;
  // The PR's head branch, said on its row when the tree is on another one —
  // the same rule that gates the actions bound to that branch. Not before the
  // git state loads, so the line doesn't flash on every open.
  const prBranch =
    gitState && viewedPr?.branch && viewedPr.branch !== gitState.branch ? viewedPr.branch : null;
  // No link then either: the compare would be the tree's branch, not the PR's.
  const commitsLink: string | null =
    webBase && unpushed === 0 && aheadCount > 0 && !prBranch
      ? aheadCount === 1 && gitState?.head_sha
        ? `${webBase}/commit/${gitState.head_sha}`
        : `${webBase}/compare/${base}...${branch}`
      : null;
  // Which sub-agent opened the PR in view, when one did — its row says so.
  const prOrigin = usePrOrigin(agent.id, viewedPr?.number);
  // A merged PR's row ends its branch: the two actions, each through the one
  // gate (branch, scope, host, work since the merge), run like any other.
  const laneEnding: LadderBodies["ending"] = ending ? { actions: ending, onRun: runAction } : null;

  // A multi-repo agent's checkout paths are addressed as `<subdir>/<path>` (as
  // in the Code tab's tree), which is how `get_file_diff` picks the checkout.
  const repoDir = agent.repos.length > 1 ? repo?.subdir : undefined;

  // The commit composer yields while the agent holds a delegation, and only
  // belongs to the lane whose tree it commits.
  const showCommit = acting && committable && !delegation;
  // Keys Fletch refuses to run git over here. While any remain, the git state is
  // stale and every action would fail, so the card replaces the body and the
  // footer steps aside.
  const blockedConfig = useAppStore((s) => s.gitBlocked[key]);
  const blocked = blockedConfig != null;

  return (
    <>
      <WorktreesNote worktrees={gitState?.worktrees} />

      {/* ── scrollable body: the lane's way to landing ── */}
      <div className={`git-body ${busy ? "busy" : ""}`}>
        {blockedConfig ? (
          <BlockedConfigCard
            keys={blockedConfig}
            busy={busy != null}
            onRemove={() => runAction("clear-config")}
          />
        ) : (
          <>
            {viewed && (
              <LaneBody
                key={viewed.id}
                agentId={agent.id}
                repoDir={repoDir}
                rows={rows}
                bodies={{
                  files: acting ? (gitState?.files ?? []) : [],
                  onRefresh: () => void fetchGitState(agent.id),
                  commitsLink: acting ? commitsLink : null,
                  commitsAhead: aheadCount,
                  pr: viewedPr,
                  prBranch,
                  origin: prOrigin,
                  checks: viewedChecks,
                  comments: acting ? laneComments : null,
                  onAddToChat: addCommentToChat,
                  ending: laneEnding,
                  review: reviewBodies,
                }}
              />
            )}

            {/* What the agent did on the focused PR by itself — under that PR's
             *  ladder, and no other lane's. Renders nothing until it has done
             *  something. */}
            {viewed?.focused && <AutopilotHistory agentId={agent.id} subdir={subdir} />}
          </>
        )}
      </div>

      {/* ── pinned footer: one row of status + action, with the commit
             message field unfolding above it only when the user opts in ── */}
      <div className="git-foot" hidden={blocked}>
        {showCommit && (
          <CommitComposer
            writing={override}
            msg={msg}
            setMsg={setMsg}
            textareaRef={commitRef}
            onRevert={revertOverride}
            // Cmd/Ctrl+Enter in the field is the main button's twin, so it
            // inherits its disabled state: the environment can't run the
            // selected action, the merge gate is shut, or a delegation holds
            // the checkout. `runAction` refuses the first of those by itself,
            // but the shortcut should be as dead as the button for all three.
            onSubmit={() => {
              if (!mainDisabled) runAction(primary.key);
            }}
          />
        )}

        <ActionBar
          // The resting note: who writes the commit message while there is one
          // to write (the ladder already says "3 files" and the button
          // "commit"); on a lane only looked at, what focusing would change.
          idle={
            showCommit ? (
              <CommitStatus
                writing={override}
                hasMsg={msg.trim().length > 0}
                onOpen={openOverride}
              />
            ) : acting ? undefined : (
              "Merge, checks and autopilot follow the focused PR"
            )
          }
          busy={busy}
          delegationLabel={delegation ? delegationLabel(delegation.kind) : null}
          autoAttempt={autoAttempt}
          notice={notice ?? delegationNotice ?? null}
          primary={primary}
          groups={groups}
          tone={tone}
          mainDisabled={mainDisabled}
          mainReason={mainReason}
          onPick={onPick}
          onRun={() => runAction(primary.key)}
        />
      </div>
    </>
  );
}
