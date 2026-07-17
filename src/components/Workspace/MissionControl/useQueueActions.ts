// MissionControl/useQueueActions.ts — the action layer (§3/§4). One surface, two
// backends: a workflow item decides through the workflow commands (wfApprove /
// wfReject via the ReviewSurface modal); an ad-hoc agent item routes through the
// SAME delegation ladder the Git panel uses (deriveState + the merge-gate +
// delegateGitAction / mergePr). No new backend commands — and never a dead
// action: any state the ladder can't map cleanly falls back to opening the
// agent's Git tab.

import { useCallback } from "react";
import { api } from "@/api";
import { appActionMessage, type GitDelegationKind } from "@/components/RightPanel/delegation";
import { describeMergeGate } from "@/components/RightPanel/mergeGate";
import { deriveState, type GitCommitAction } from "@/components/RightPanel/primaryActions";
import { useAppStore } from "@/store";
import { gitKey } from "@/store/git";
import type { ReviewItem } from "./queue";

/** Composer scaffold seeded for "request changes" on an ad-hoc agent item — an
 *  editable starting point (like the PR-comment "→ chat" seed), not a sent
 *  message. The user refines it in the agent's chat before sending. When the
 *  card's signal lives in a secondary repo, the seed names it — the composer is
 *  agent-level, so the repo scope must ride in the prompt itself. */
function requestChangesSeed(subdir: string | undefined): string {
  const scope = subdir ? ` in the \`${subdir}\` repo` : "";
  return `Please make the following changes${scope} before this is ready:\n\n- `;
}

/** Map the user's sticky commit mode to the delegation it drives — the same
 *  triple the Git panel's `changes` state uses. */
function commitDelegation(mode: GitCommitAction): { kind: GitDelegationKind; trigger: string } {
  switch (mode) {
    case "agent-commit":
      return { kind: "commit", trigger: "commit" };
    case "agent-commit-push":
      return { kind: "commit-push", trigger: "commit-push" };
    default:
      return { kind: "commit-pr", trigger: "commit-pr" };
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
  const delegateGitAction = useAppStore((s) => s.delegateGitAction);
  const setLastError = useAppStore((s) => s.setLastError);
  const dismissReviewItem = useAppStore((s) => s.dismissReviewItem);

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
  // holds compact shortstats), classify it exactly as the Git panel does, and
  // delegate the matching action — or open the tab when there's no clean move.
  // `subdir` scopes everything to the repo whose signal the card shows — a
  // secondary repo's failing PR must never dispatch an action on the primary.
  const approveAgent = useCallback(
    async (agentId: string, subdir: string | undefined) => {
      await fetchGitState(agentId, subdir);
      const s = useAppStore.getState();
      const key = gitKey(agentId, subdir);
      const git = s.gitStates[key] ?? null;
      const pr = s.prStates[key] ?? null;
      const checks = s.prChecks[key] ?? null;
      const base = git?.parent_branch || "main";
      // Trigger builder scoped to this repo: a secondary adds `repo="<subdir>"`
      // so the agent works in that sibling checkout, not the primary (mirrors
      // useGitActions' trigger).
      const trigger = (name: string, params?: Record<string, string>) =>
        appActionMessage(name, subdir ? { ...params, repo: subdir } : params);
      const state = deriveState(git, pr);
      switch (state) {
        case "changes": {
          // With a PR already open, "open PR" degrades to "push" — that's what
          // updates the existing PR (mirrors primaryActions' changes state).
          const mode: GitCommitAction =
            pr?.state === "open" && s.gitCommitAction === "agent-commit-pr"
              ? "agent-commit-push"
              : s.gitCommitAction;
          const { kind, trigger: name } = commitDelegation(mode);
          delegateGitAction(
            agentId,
            kind,
            trigger(name, kind === "commit-pr" ? { base } : undefined),
            subdir,
          );
          return;
        }
        case "pushed":
          delegateGitAction(agentId, "open-pr", trigger("open-pr", { base }), subdir);
          return;
        case "conflicts":
          delegateGitAction(agentId, "resolve", trigger("resolve-conflicts"), subdir);
          return;
        case "pr-open": {
          const gate = describeMergeGate(checks?.merge_state ?? null, {
            checksFailed: checks?.required_failing.length ?? 0,
            mergeable: pr?.mergeable ?? false,
          });
          if (gate.mergeAllowed) {
            await mergePr(agentId, subdir);
            return;
          }
          if (gate.situation === "checks-failing") {
            delegateGitAction(
              agentId,
              "fix-checks",
              trigger("fix-checks", { failing: (checks?.required_failing ?? []).join(", ") }),
              subdir,
            );
            return;
          }
          if (gate.needsUpdate) {
            delegateGitAction(agentId, "update-branch", trigger("update-branch", { base }), subdir);
            return;
          }
          break;
        }
      }
      // clean / merged / pr-closed / review-required / loading — nothing to
      // delegate: open the tab so the decision is the user's, never a dead key.
      openAgentGit(agentId);
    },
    [fetchGitState, delegateGitAction, mergePr, openAgentGit],
  );

  const enter = useCallback(
    (item: ReviewItem) => {
      if (item.kind === "workflow" && item.runId) openReview(item.runId);
      else if (item.agent) openAgentGit(item.agent.id);
    },
    [openReview, openAgentGit],
  );

  const approve = useCallback(
    (item: ReviewItem) => {
      if (item.kind === "workflow" && item.runId) {
        void api.wfApprove(item.runId).catch((e) => setLastError(`Approve failed: ${e}`));
        return;
      }
      if (item.agent) void approveAgent(item.agent.id, item.prSubdir);
    },
    [approveAgent, setLastError],
  );

  const requestChanges = useCallback(
    (item: ReviewItem) => {
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
