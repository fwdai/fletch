// When the Devices page mints the next pairing invite on its own. Its own
// module because it is the page's one decision worth a test: the hook around
// it is timers and a call.

import type { PairingInvite, PairingPreset } from "@/api";

export interface WindowFacts {
  /** The listener is up and nothing blocks pairing (`status.error`). */
  canPair: boolean;
  invite: Pick<PairingInvite, "expiresAt" | "preset"> | null;
  /** The host closed the window itself: too many pairing requests. */
  closed: boolean;
  /** What the next code should grant. */
  preset: PairingPreset;
  /** A device paired since the invite was minted, so its code is spent. */
  redeemed: boolean;
  /** Epoch ms. */
  now: number;
}

/** Whether to mint a new invite now: there is none, or the one shown has
 *  lapsed, was used, or grants another preset than the one chosen.
 *
 *  Never after the host closed the window itself. That close is the
 *  protocol's bound on a party in the middle rolling for a matching code
 *  (docs/remote-protocol.md, "Confirmed pairing": five tries per window, and
 *  every further window takes someone at the Mac) — reopening it at once
 *  would hand whoever is rolling five more, for as long as the page is open.
 *  The card's "New code" is that someone. */
export function needsInvite(facts: WindowFacts): boolean {
  if (!facts.canPair || facts.closed) return false;
  if (!facts.invite || facts.redeemed) return true;
  if (facts.invite.preset !== facts.preset) return true;
  const at = Date.parse(facts.invite.expiresAt);
  return Number.isNaN(at) || at <= facts.now;
}
