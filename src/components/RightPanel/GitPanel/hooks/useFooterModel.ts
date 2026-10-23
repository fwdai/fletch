import { useCallback } from "react";
import type { GitState, MergeState, PrChecks, PrComments, PrState } from "@/api";
import type { ActionTone } from "@/components/RightPanel/primaryActions";
import { describeMergeGate } from "@/mergeGate";
import { hasWorkSinceMerge } from "@/readiness";
import { useAppStore } from "@/store";
import { activeEntry } from "@/store/capabilities";
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
  gitState: GitState | null;
  prState: PrState | null;
  checks: PrChecks | null;
  mergeState: MergeState | null;
  comments: PrComments | null;
  base: string;
  customActive: boolean;
  delegationActive: boolean;
  busy: boolean;
  githubConnected: boolean;
  primaryCheckout: boolean;
  /** The local reviewer's menu entries; absent on a lane the footer doesn't
   *  act on. */
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
  const env = useAppStore(activeEntry);

  // The merge gate's `checksFailed` means *failing required checks*; the
  // ladder, the PR row and this all derive it the same way (`readiness.ts`).
  const { mergeAllowed } = describeMergeGate(checks ? mergeState : null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: prState?.mergeable ?? "unknown",
  });
  const laneBranch = prState?.branch ?? null;
  const checkoutBranch = gitState?.branch || null;
  const workSinceMerge = gitState != null && hasWorkSinceMerge(gitState);
  const { delegationActive, busy, primaryCheckout } = input;
  const gate = useCallback(
    (key: string) =>
      gateAction(key, {
        hostRefusal: (k) => actionGateReason(env, k),
        laneBranch,
        checkoutBranch,
        mergeAllowed,
        delegationActive,
        busy,
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
      primaryCheckout,
      workSinceMerge,
    ],
  );

  const footer =
    mode === "focus"
      ? focusFooter()
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
