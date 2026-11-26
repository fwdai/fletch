import { useCallback } from "react";
import type { GitState, MergeState, PrChecks, PrComments, PrState } from "@/api";
import type { ActionTone } from "@/components/RightPanel/primaryActions";
import { describeMergeGate } from "@/mergeGate";
import { hasWorkSinceMerge } from "@/readiness";
import { useAppStore } from "@/store";
import { useHostEntry } from "@/store/capabilities";
import { type Gate, gateAction } from "../actionGate";
import { actionGateReason } from "../actionGates";
import {
  applyGate,
  buildFooter,
  endingActions,
  type Footer,
  type FooterAction,
  focusFooter,
  type ReviewFooterInput,
} from "../footerModel";

/** The footer as the panel renders it: `buildFooter` (or `focusFooter` for a
 *  lane the panel only looks at) with the one action gate applied to every
 *  action; the merged PR row's ending actions through the same gate; and that
 *  gate for the dispatch to re-run. */
export function useFooterModel(input: {
  /** `acting`: the lane the footer's actions apply to. `focus`: another PR of
   *  the checkout, which can only be focused or viewed on GitHub. */
  mode: "acting" | "focus";
  /** The working tree is on the lane's branch: its footer offers the tree's
   *  actions too. Off it, an acting lane's footer is its PR's alone. */
  tree: boolean;
  gitState: GitState | null;
  prState: PrState | null;
  checks: PrChecks | null;
  mergeState: MergeState | null;
  comments: PrComments | null;
  base: string;
  customActive: boolean;
  delegationActive: boolean;
  busy: boolean;
  /** A local review is queued or running on the checkout, whichever lane is
   *  in view. */
  reviewActive: boolean;
  /** The agent is mid-turn or still starting. */
  agentWorking: boolean;
  githubConnected: boolean;
  primaryCheckout: boolean;
  /** The local reviewer's menu entries; absent off the lane whose tree it
   *  reads. */
  reviewMenu?: ReviewFooterInput;
}): Footer & {
  tone: ActionTone;
  mainDisabled: boolean;
  mainReason: string | null;
  /** Archive and delete-branch, gated, for a merged PR the footer acts on. */
  ending: FooterAction[] | null;
  gate: (key: string) => Gate | null;
} {
  const { mode, gitState, prState, checks, mergeState, comments, base, customActive } = input;
  const commitMode = useAppStore((s) => s.gitCommitAction);
  const env = useHostEntry();

  // The merge gate's `checksFailed` means *failing required checks*; the
  // ladder, the PR row and this all derive it the same way (`readiness.ts`).
  const { mergeAllowed } = describeMergeGate(checks ? mergeState : null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: prState?.mergeable ?? "unknown",
  });
  const laneBranch = prState?.branch ?? null;
  const checkoutBranch = gitState?.branch || null;
  const workSinceMerge = gitState != null && hasWorkSinceMerge(gitState);
  const { delegationActive, busy, primaryCheckout, reviewActive, agentWorking } = input;
  const treeDirty = (gitState?.files.length ?? 0) > 0;
  const gate = useCallback(
    (key: string) =>
      gateAction(key, {
        hostRefusal: (k) => actionGateReason(env, k),
        laneBranch,
        checkoutBranch,
        mergeAllowed,
        delegationActive,
        busy,
        reviewActive,
        agentWorking,
        treeDirty,
        primaryCheckout,
        workSinceMerge,
      }),
    [
      env,
      laneBranch,
      checkoutBranch,
      mergeAllowed,
      delegationActive,
      busy,
      reviewActive,
      agentWorking,
      treeDirty,
      primaryCheckout,
      workSinceMerge,
    ],
  );

  const footer =
    mode === "focus"
      ? focusFooter({ pr: prState, tree: input.tree })
      : buildFooter({
          git: gitState,
          pr: prState,
          checks,
          mergeState,
          comments,
          base,
          commitMode,
          customActive,
          githubConnected: input.githubConnected,
          // A local-only repo (no origin) can't push/PR until published. GitState
          // is null while loading — assume an origin then so the primary doesn't
          // flicker to "Publish" before the first read lands.
          hasOrigin: gitState?.has_origin ?? true,
          reviewMenu: input.reviewMenu,
          lane: { tree: input.tree, focused: true },
        });
  const { primary, groups, mainDisabled, mainReason } = applyGate(footer, gate);
  const ending =
    mode === "acting" && prState?.state === "merged"
      ? applyGate({ primary, groups: [{ items: endingActions() }] }, gate).groups[0].items
      : null;
  return {
    primary,
    groups,
    tone: primary.tone ?? "accent",
    mainDisabled,
    mainReason,
    ending,
    gate,
  };
}
