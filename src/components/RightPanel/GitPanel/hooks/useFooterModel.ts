import { useCallback } from "react";
import type { GitState, MergeState, PrChecks, PrComments, PrState } from "@/api";
import type { ActionTone } from "@/components/RightPanel/primaryActions";
import { describeMergeGate } from "@/mergeGate";
import { useAppStore } from "@/store";
import { activeEntry } from "@/store/capabilities";
import { type Gate, gateAction } from "../actionGate";
import { applyGate, buildFooter, type Footer, focusFooter } from "../footerModel";

/** The footer as the panel renders it: `buildFooter` (or `focusFooter` for a
 *  lane the panel only looks at) with the one action gate applied to every
 *  action, plus that gate for the dispatch to re-run. */
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
  githubConnected: boolean;
  primaryCheckout: boolean;
}): Footer & {
  tone: ActionTone;
  mainDisabled: boolean;
  mainReason: string | null;
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
  const { delegationActive, primaryCheckout } = input;
  const gate = useCallback(
    (key: string) =>
      gateAction(key, {
        env,
        laneBranch,
        checkoutBranch,
        mergeAllowed,
        delegationActive,
        primaryCheckout,
      }),
    [env, laneBranch, checkoutBranch, mergeAllowed, delegationActive, primaryCheckout],
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
        });
  const { primary, groups, mainDisabled, mainReason } = applyGate(footer, gate);
  return { primary, groups, tone: primary.tone ?? "accent", mainDisabled, mainReason, gate };
}
