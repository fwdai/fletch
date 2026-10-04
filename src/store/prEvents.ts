// The host PR watcher's three events folded into the PR slices. Each writes
// one checkout's key — `checkoutKey(agent_id, subdir)`, the same key the seed
// (`loadAllPrStatus`) writes — and stamps it, so a seed or one-shot read
// already in flight, which observed the PR before this change, can't land
// afterwards and roll it back (a merged badge flipping back to open).

import type {
  PrChecksChangedEvent,
  PrStateChangedEvent,
  PrThreadsChangedEvent,
} from "@/api/types/pr";
import { checkoutKey, type GitSlice } from "./git";
import { type PrSlice, stampPrWrite } from "./prWriteOrder";

/** The event's checkout key, stamped as just written in `slice`. */
function claim(slice: PrSlice, e: { agent_id: string; subdir?: string | null }): string {
  const key = checkoutKey(e.agent_id, e.subdir ?? undefined);
  stampPrWrite(slice, key);
  return key;
}

export function applyPrStateChanged(
  s: Pick<GitSlice, "prStates">,
  e: PrStateChangedEvent,
): Pick<GitSlice, "prStates"> {
  const key = claim("prStates", e);
  return { prStates: { ...s.prStates, [key]: e.state } };
}

export function applyPrChecksChanged(
  s: Pick<GitSlice, "prChecks">,
  e: PrChecksChangedEvent,
): Pick<GitSlice, "prChecks"> {
  const key = claim("prChecks", e);
  return { prChecks: { ...s.prChecks, [key]: e.checks } };
}

export function applyPrThreadsChanged(
  s: Pick<GitSlice, "prComments">,
  e: PrThreadsChangedEvent,
): Pick<GitSlice, "prComments"> {
  const key = claim("prComments", e);
  return { prComments: { ...s.prComments, [key]: e.comments } };
}
