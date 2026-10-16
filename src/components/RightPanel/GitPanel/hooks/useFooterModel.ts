import type { GitState, MergeState, PrChecks, PrComments, PrState } from "@/api";
import type { ActionTone } from "@/components/RightPanel/primaryActions";
import { describeMergeGate } from "@/mergeGate";
import { useAppStore } from "@/store";
import { useGateFor } from "@/store/capabilities";
import { ACTION_GATES } from "../actionGates";
import { buildFooter, type Footer } from "../footerModel";
import { mainActionState } from "../mainAction";

/** The footer as the panel renders it: `buildFooter`'s primary and groups off
 *  the live state, plus whether the primary can run here and why not. */
export function useFooterModel(input: {
  gitState: GitState | null;
  prState: PrState | null;
  checks: PrChecks | null;
  mergeState: MergeState | null;
  comments: PrComments | null;
  base: string;
  customActive: boolean;
  delegationActive: boolean;
  githubConnected: boolean;
}): Footer & { tone: ActionTone; mainDisabled: boolean; mainReason: string | null } {
  const { gitState, prState, checks, mergeState, comments, base, customActive, delegationActive } =
    input;
  const commitMode = useAppStore((s) => s.gitCommitAction);

  const { primary, groups } = buildFooter({
    git: gitState,
    pr: prState,
    checks,
    mergeState,
    comments,
    base,
    commitMode,
    customActive,
    githubConnected: input.githubConnected,
    // A local-only repo (no origin) can't push/PR until published. GitState is
    // null while loading — assume an origin then so the primary doesn't
    // flicker to "Publish" before the first read lands.
    hasOrigin: gitState?.has_origin ?? true,
  });

  // Whether the environment the user is driving can run the primary at all:
  // null locally and on a host that answers its op, a reason against one that
  // doesn't — where the button has to be dead rather than dispatch a call that
  // comes back `unknown op`.
  const actionGate = useGateFor(ACTION_GATES[primary.key]);

  // The merge gate's `checksFailed` means *failing required checks*; the
  // ladder, the PR row and this all derive it the same way (`readiness.ts`).
  const { mergeAllowed } = describeMergeGate(checks ? mergeState : null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: prState?.mergeable ?? "unknown",
  });
  const { disabled: mainDisabled, reason: mainReason } = mainActionState({
    effectiveKey: primary.key,
    delegationActive,
    mergeAllowed,
    actionGate,
    prBranch: prState?.branch ?? null,
    checkoutBranch: gitState?.branch || null,
  });

  return { primary, groups, tone: primary.tone ?? "accent", mainDisabled, mainReason };
}
