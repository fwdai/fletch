// Which of the Git panel's actions the environment the user is driving can be
// asked to run — by the key `runAction` dispatches.
//
// The table sits beside the panel rather than inside a hook because both halves
// of the panel need the same answer: `useActionBarModel` in render (through
// `useGateFor`) to leave the button dead, and `useGitActions.runAction` outside
// render to refuse the dispatch. Disabling the button alone was not enough —
// the commit composer's Cmd/Ctrl+Enter calls `runAction` directly, so an older
// host still got the call it would answer `unknown op`.

import { type GateName, gateReason } from "@/store/capabilities";
import { activeEnvironment, type EnvironmentEntry } from "@/store/environments";

/** The panel actions that call a git op on the environment's host, by the key
 *  `runAction` dispatches. Everything else the menu offers is either local
 *  (`view-pr`), delegated to the coding agent (`agent-*`), or an op that has
 *  been on the wire since v1 (`push`, `commit-*`, `open-pr`, `archive`) — so
 *  only these can be refused by the host the user is driving. */
export const ACTION_GATES: Partial<Record<string, GateName>> = {
  merge: "mergePr",
  pull: "pull",
  rebase: "rebase",
  stash: "stash",
  discard: "discardChanges",
  abort: "abortMerge",
  "clear-config": "clearCheckoutConfig",
  "delete-branch": "deleteBranch",
};

/** Why `env` cannot run the panel action `key`, or null when it can — which is
 *  every action locally, and every action whose op the host answers (including
 *  the keys that call no gated op at all). Pure, so the mapping is testable
 *  without a store or a host. */
export function actionGateReason(env: EnvironmentEntry, key: string): string | null {
  const gate = ACTION_GATES[key];
  return gate ? gateReason(env, gate) : null;
}

/** [`actionGateReason`] for the environment the user is driving, for the
 *  dispatch path — which runs outside render and so cannot use a hook. */
export function activeActionGateReason(key: string): string | null {
  return actionGateReason(activeEnvironment(), key);
}
