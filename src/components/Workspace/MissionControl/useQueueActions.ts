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
import { appActionMessage } from "@/delegation";
import { type LadderContext, nextRung } from "@/readiness";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
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
  // holds compact shortstats), then ask the shared ladder what to do. The
  // classification lives in `readiness.ts`, so this surface and the Git panel
  // cannot disagree about what's wrong — they used to, each having its own copy.
  // `subdir` scopes everything to the repo whose signal the card shows — a
  // secondary repo's failing PR must never dispatch an action on the primary.
  const approveAgent = useCallback(
    async (agentId: string, subdir: string | undefined) => {
      await fetchGitState(agentId, subdir);
      const s = useAppStore.getState();
      const key = checkoutKey(agentId, subdir);
      const git = s.gitStates[key] ?? null;
      const input = {
        git,
        pr: s.prStates[key] ?? null,
        checks: s.prChecks[key] ?? null,
        comments: s.prComments[key] ?? null,
      };
      // Approving a checkout autopilot gave up on IS the human "try again" it was
      // waiting for, so clear the stuck state (and its spent budget) as part of
      // the gesture. Without this the user would retry by hand while autopilot
      // stayed stopped forever — and `stuck` is deliberately only clearable by a
      // human, so nothing else would ever do it.
      if (s.autopilot[key]?.stuck) s.resumeAutopilot(key);

      const rung = nextRung(input, {
        base: git?.parent_branch || "main",
        commitMode: commitMode(s.gitCommitAction),
      });

      switch (rung.do) {
        case "delegate":
          // Scope the trigger to this repo: a secondary adds `repo="<subdir>"` so
          // the agent works in that sibling checkout, not the primary (mirrors
          // useGitActions' trigger).
          delegateAction(
            agentId,
            rung.kind,
            appActionMessage(rung.action, subdir ? { ...rung.params, repo: subdir } : rung.params),
            subdir,
          );
          return;
        case "merge":
          await mergePr(agentId, subdir);
          return;
        default:
          // escalate / wait / landed / ready — nothing to delegate. Open the tab
          // so the decision is the user's, never a dead key.
          openAgentGit(agentId);
      }
    },
    [fetchGitState, delegateAction, mergePr, openAgentGit],
  );

  // Fan-out "Update all": dispatch the existing `update-branch` delegation to
  // every affected agent, each scoped to its own checkout. Running agents queue
  // the trigger and idle ones start immediately — either way each flips into its
  // delegated/running state through the same machinery the Git panel uses, so no
  // new progress UI is needed.
  const updateAll = useCallback(
    (item: ReviewItem) => {
      const fanout = item.fanout;
      if (!fanout) return;
      for (const a of fanout.agents) {
        const trigger = appActionMessage(
          "update-branch",
          a.subdir ? { base: fanout.base, repo: a.subdir } : { base: fanout.base },
        );
        delegateAction(a.agentId, "update-branch", trigger, a.subdir);
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
      if (item.agent) void approveAgent(item.agent.id, item.prSubdir);
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
