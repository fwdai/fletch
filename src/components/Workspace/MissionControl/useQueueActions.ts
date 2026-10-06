// MissionControl/useQueueActions.ts — the action layer (§3/§4). One surface, two
// backends: a workflow item decides through the workflow commands (wfApprove /
// wfReject via the ReviewSurface modal); an ad-hoc agent item routes through the
// shared remediation ladder (`readiness.ts`) that the Git panel classifies from
// too — this surface holds no copy of it. No new backend commands, and never a
// dead action: any rung that isn't an agent's to run falls back to opening the
// agent's Git tab.

import { open } from "@tauri-apps/plugin-shell";
import { useCallback } from "react";
import { api } from "@/api";
import type { GitCommitAction } from "@/components/RightPanel/primaryActions";
import { type LadderContext, nextRung } from "@/readiness";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { checkoutKey } from "@/store/git";
import { BUCKET, type ReviewItem } from "./queue";

/** Composer scaffold seeded for "request changes" on an ad-hoc agent item — an
 *  editable starting point (like the PR-comment "→ chat" seed), not a sent
 *  message. The user refines it in the agent's chat before sending. When the
 *  card's signal lives in a secondary repo, the seed names it — the composer is
 *  agent-level, so the repo scope must ride in the prompt itself. */
function requestChangesSeed(subdir: string | undefined): string {
  const scope = subdir ? ` in the \`${subdir}\` repo` : "";
  return `Please make the following changes${scope} before this is ready:\n\n- `;
}

/** The panel's sticky commit setting as the ladder's neutral triple — the ladder
 *  is kept free of panel vocabulary so it can move to Rust unchanged. */
function commitMode(action: GitCommitAction): LadderContext["commitMode"] {
  switch (action) {
    case "agent-commit":
      return "commit";
    case "agent-commit-push":
      return "commit-push";
    default:
      return "commit-pr";
  }
}

export interface QueueActions {
  /** ↵ — open the item's review (workflow: the ReviewSurface modal; agent: its
   *  Git tab). */
  enter: (item: ReviewItem) => void;
  /** a — approve / advance (workflow: wfApprove; agent: the delegation ladder). */
  approve: (item: ReviewItem) => void;
  /** r — request changes (workflow: the reject form in the modal; agent: seed
   *  its composer). */
  requestChanges: (item: ReviewItem) => void;
  /** The dismiss affordance — hides the card until its signal changes. */
  dismiss: (item: ReviewItem) => void;
}

/** Build the queue's action handlers. `openReview` hands a workflow run id up to
 *  the pane, which mounts the shared ReviewSurface over it. */
export function useQueueActions(openReview: (runId: string) => void): QueueActions {
  const selectAgent = useAppStore((s) => s.selectAgent);
  const setRightPanelTab = useAppStore((s) => s.setRightPanelTab);
  const seedComposer = useAppStore((s) => s.seedComposer);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const mergePr = useAppStore((s) => s.mergePr);
  const delegateAction = useAppStore((s) => s.delegateAction);
  const focusPr = useAppStore((s) => s.focusPr);
  const setLastError = useAppStore((s) => s.setLastError);
  const dismissReviewItem = useAppStore((s) => s.dismissReviewItem);
  const mergePrGate = useGate("mergePr");

  // Send the user to the agent's Git tab — the honest fallback whenever an
  // action can't be mapped to a single clean gesture.
  const openAgentGit = useCallback(
    (agentId: string) => {
      selectAgent(agentId);
      setRightPanelTab(agentId, "git");
    },
    [selectAgent, setRightPanelTab],
  );

  // The ad-hoc "approve" ladder: pull authoritative git/PR state (the queue only
  // holds compact shortstats), then ask the shared ladder what to do. The
  // classification lives in `readiness.ts`, so this surface and the Git panel
  // cannot disagree about what's wrong — they used to, each having its own copy.
  // `subdir` scopes everything to the repo whose signal the card shows — a
  // secondary repo's failing PR must never dispatch an action on the primary.
  // `prNumber` is the PR a PR card shows, which may be a sibling of the
  // checkout's focused one (a failing one): the ladder reads, and the host's
  // delegations target, the focused PR, so focus moves to it first.
  const approveAgent = useCallback(
    async (agentId: string, subdir: string | undefined, prNumber: number | undefined) => {
      const key = checkoutKey(agentId, subdir);
      const before = useAppStore.getState();
      if (
        prNumber != null &&
        before.prStates[key]?.number !== prNumber &&
        before.prSets[key]?.some((e) => e.state.number === prNumber)
      ) {
        await focusPr(agentId, prNumber, subdir);
        // Refused (the error is already reported): acting now would act on a
        // different PR than the card showed, so hand the decision over.
        if (useAppStore.getState().prStates[key]?.number !== prNumber) {
          openAgentGit(agentId);
          return;
        }
      }
      await fetchGitState(agentId, subdir);
      const s = useAppStore.getState();
      const git = s.gitStates[key] ?? null;
      const input = {
        git,
        pr: s.prStates[key] ?? null,
        checks: s.prChecks[key] ?? null,
        comments: s.prComments[key] ?? null,
      };
      const rung = nextRung(input, {
        base: git?.parent_branch || "main",
        commitMode: commitMode(s.gitCommitAction),
      });

      switch (rung.do) {
        case "delegate":
          // Scoped to this repo: the host adds `repo="<subdir>"` for a
          // secondary so the agent works in that sibling checkout, not the
          // primary.
          await delegateAction(agentId, rung.action, rung.params, subdir);
          return;
        case "merge":
          // A host from before `merge_pr` says so instead of dispatching a call
          // that comes back `unknown op`.
          if (mergePrGate) {
            setLastError(mergePrGate);
            return;
          }
          await mergePr(agentId, subdir);
          return;
        default:
          // escalate / wait / landed / ready — nothing to delegate. Open the tab
          // so the decision is the user's, never a dead key.
          openAgentGit(agentId);
      }
    },
    [fetchGitState, focusPr, delegateAction, mergePr, mergePrGate, setLastError, openAgentGit],
  );

  // Fan-out "Update all": dispatch the existing `update-branch` delegation to
  // every affected agent, each scoped to its own checkout. The host queues the
  // trigger for running agents and starts idle ones immediately — either way
  // each flips into its delegated/running state through the same machinery the
  // Git panel uses, so no new progress UI is needed.
  const updateAll = useCallback(
    (item: ReviewItem) => {
      const fanout = item.fanout;
      if (!fanout) return;
      for (const a of fanout.agents) {
        void delegateAction(a.agentId, "update-branch", { base: fanout.base }, a.subdir);
      }
    },
    [delegateAction],
  );

  const enter = useCallback(
    (item: ReviewItem) => {
      if (item.kind === "fanout") {
        if (item.fanout) void open(item.fanout.merged.url);
        return;
      }
      if (item.kind === "workflow" && item.runId) openReview(item.runId);
      else if (item.agent) openAgentGit(item.agent.id);
    },
    [openReview, openAgentGit],
  );

  const approve = useCallback(
    (item: ReviewItem) => {
      if (item.kind === "fanout") {
        updateAll(item);
        return;
      }
      if (item.kind === "workflow" && item.runId) {
        void api.wfApprove(item.runId).catch((e) => setLastError(`Approve failed: ${e}`));
        return;
      }
      // Only a PR card's PR is worth moving the focus for: approving a card
      // about unseen results never moves the Git panel's focus.
      const prNumber = item.bucket === BUCKET.pr ? item.pr?.number : undefined;
      if (item.agent) void approveAgent(item.agent.id, item.prSubdir, prNumber);
    },
    [approveAgent, updateAll, setLastError],
  );

  const requestChanges = useCallback(
    (item: ReviewItem) => {
      // A fan-out card has no "request changes" gesture — its only action is
      // Update all (bound to `a`). `r` is a no-op here.
      if (item.kind === "fanout") return;
      // Workflow reject needs a note — that lives in the ReviewSurface's reject
      // form, so open the same modal rather than rejecting blind.
      if (item.kind === "workflow" && item.runId) {
        openReview(item.runId);
        return;
      }
      if (item.agent) {
        seedComposer(item.agent.id, requestChangesSeed(item.prSubdir));
        selectAgent(item.agent.id);
      }
    },
    [openReview, seedComposer, selectAgent],
  );

  const dismiss = useCallback(
    (item: ReviewItem) => dismissReviewItem(item.id, item.signature),
    [dismissReviewItem],
  );

  return { enter, approve, requestChanges, dismiss };
}
