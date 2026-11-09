// Pairing with a host picked from the nearby list: the decisions, apart from
// the hook that drives them, so they can be tested on their own.

import { type NearbyHost, nearbyTarget } from "@/remote/nearby";
import type { PairTarget } from "@/remote/registry";
import { NO_CONFIRMER_ERROR } from "@/remote/types";

/** Where the flow is. `asking` is a confirmed pairing in flight — `code` is
 *  the six digits both screens show, once the host has answered — and `code`
 *  is a host that cannot confirm, waiting for the code its screen shows. */
export type NearbyStep =
  | { kind: "idle" }
  | { kind: "asking"; host: NearbyHost; code: string | null }
  | { kind: "code"; host: NearbyHost; sending: boolean };

/** The nearby hosts worth offering: the ones this Mac has not paired with
 *  yet. A paired host is already in the list below, with its own Forget. */
export function unpairedNearby(nearby: NearbyHost[], paired: { hostKey: string }[]): NearbyHost[] {
  const known = new Set(paired.map((h) => h.hostKey));
  return nearby.filter((h) => !known.has(h.hostKey));
}

/** The target for one attempt at `host`: by confirmation, or with the code
 *  its screen shows (spaces dropped, upper-cased: the host mints `A-Z2-9`).
 *  Null when the key it announced names no host. */
export function pairTargetFor(host: NearbyHost, code?: string): PairTarget | null {
  const target = nearbyTarget(host);
  if (!target) return null;
  return code === undefined
    ? { ...target, confirm: true }
    : { ...target, pairingToken: code.replace(/\s+/g, "").toUpperCase() };
}

/** Where a failed attempt leaves the flow, and what to say. A host with
 *  nobody at its screen says so in the protocol's exact words, which is the
 *  signal to ask for its code rather than an error. A refused code stays on
 *  the code, for another try; anything else goes back to the list. */
export function afterFailure(
  host: NearbyHost,
  withCode: boolean,
  message: string,
): { step: NearbyStep; error: string | null } {
  if (!withCode && message === NO_CONFIRMER_ERROR) {
    return { step: { kind: "code", host, sending: false }, error: null };
  }
  return {
    step: withCode ? { kind: "code", host, sending: false } : { kind: "idle" },
    error: message,
  };
}
