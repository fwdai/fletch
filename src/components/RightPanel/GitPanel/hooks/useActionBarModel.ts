import { useEffect, useState } from "react";
import type { GitState, MergeState, PrChecks, PrState } from "@/api";
import type { SplitActionItem } from "@/components/RightPanel/GitPanel/SplitAction";
import { describeMergeGate } from "@/components/RightPanel/mergeGate";
import {
  type ActionTone,
  type GitPanelState,
  isCommitAction,
  primaryFor,
  secondaryFor,
} from "@/components/RightPanel/primaryActions";
import { useAppStore } from "@/store";

/** Builds the split-button model for the current state: the action counts, the
 *  primary/secondary actions, the menu `items`, and the selection bookkeeping
 *  (default = primary; reset on state/agent change; orphan-safe `effectiveKey`).
 *  Also resolves the CTA tone and disabled gate. */
export function useActionBarModel(input: {
  agentId: string;
  panelState: GitPanelState;
  gitState: GitState | null;
  prState: PrState | null;
  checks: PrChecks | null;
  mergeState: MergeState | null;
  prOpen: boolean;
  base: string;
  customActive: boolean;
  delegationActive: boolean;
}) {
  const {
    agentId,
    panelState,
    gitState,
    prState,
    checks,
    mergeState,
    prOpen,
    base,
    customActive,
    delegationActive,
  } = input;

  const gitCommitAction = useAppStore((s) => s.gitCommitAction);
  const setGitCommitAction = useAppStore((s) => s.setGitCommitAction);

  const behind = gitState?.behind ?? 0;
  const mergeable = prState?.mergeable ?? false;

  const counts = {
    files: gitState?.files.length ?? 0,
    ahead: gitState?.ahead ?? 0,
    behind,
    unpushed: gitState?.unpushed ?? 0,
    prNumber: prState?.number,
    base,
    customActive,
    mergeable,
    mergeState,
    checksFailed: checks?.failed ?? 0,
    commitAction: gitCommitAction,
    prOpen,
  };
  const primary = primaryFor(panelState, counts);
  const secondary = secondaryFor(panelState, counts);

  // All actions for this state, primary first. The main button shows whichever
  // is currently selected; the default selection is the primary. A secondary
  // candidate that duplicates the primary is dropped (pr-open lists Merge
  // unconditionally so it stays reachable from any merge_state).
  const items: SplitActionItem[] = [
    { key: primary.key, label: primary.label, icon: primary.icon },
    ...secondary
      .filter((s) => s.key !== primary.key)
      .map((s) => ({ key: s.key, label: s.label, icon: s.icon, kbd: s.kbd })),
  ];

  // Selected action: defaults to the primary, resets whenever the state (or the
  // clean-state primary, which flips with `behind`) changes, and on agent swap.
  const [selectedKey, setSelectedKey] = useState(primary.key);
  useEffect(() => {
    setSelectedKey(primary.key);
  }, [panelState, primary.key, agentId]);

  // `selectedKey` can be orphaned when a background poll removes its menu item
  // *without* changing `primary.key` — e.g. `mergeable` flips true, dropping
  // "agent-update-branch" from the menu while the primary stays "merge", so the
  // reset effect above doesn't fire. Fall back to the primary (which is always
  // `items[0]`, what the button then displays) so the displayed action, its
  // tone/enabled state, and the dispatched action all stay in agreement.
  const effectiveKey = items.some((i) => i.key === selectedKey) ? selectedKey : primary.key;

  // The CTA's main button is disabled while loading git state, while the agent
  // holds a delegation, and when Merge is selected but the merge gate isn't
  // open. Gate semantics live in describeMergeGate (spec §6).
  const { mergeAllowed } = describeMergeGate(checks ? mergeState : null, {
    checksFailed: checks?.failed ?? 0,
    mergeable,
  });
  const mainDisabled =
    effectiveKey === "loading" || delegationActive || (effectiveKey === "merge" && !mergeAllowed);
  // Tone applies only when the selected action is the state's primary; picking
  // an alternate from the menu falls back to the neutral accent fill.
  const tone: ActionTone = effectiveKey === primary.key ? (primary.tone ?? "accent") : "accent";

  const onSelectAction = (key: string) => {
    setSelectedKey(key);
    // Picking a commit mode is sticky: it becomes the default primary in every
    // workspace until the user picks another.
    if (isCommitAction(key)) setGitCommitAction(key);
  };

  return { primary, items, effectiveKey, tone, mainDisabled, onSelectAction };
}
