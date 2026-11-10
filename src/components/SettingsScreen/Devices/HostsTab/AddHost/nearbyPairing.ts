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
 *  yet. A paired host is already in the list above, with its own Forget. */
export function unpairedNearby(nearby: NearbyHost[], paired: { hostKey: string }[]): NearbyHost[] {
  const known = new Set(paired.map((h) => h.hostKey));
  return nearby.filter((h) => !known.has(h.hostKey));
}

/** The host a pairing is in flight with, if any. */
export const hostOf = (step: NearbyStep): NearbyHost | null =>
  step.kind === "idle" ? null : step.host;

/** The rows the list shows: the nearby hosts not paired yet, and always the
 *  one a pairing is in flight with, in the place it had. Its row carries the
 *  code both screens show, so it must not leave because one browse missed
 *  its announcement. */
export function listedNearby(
  nearby: NearbyHost[],
  paired: { hostKey: string }[],
  step: NearbyStep,
): NearbyHost[] {
  const rows = unpairedNearby(nearby, paired);
  const active = hostOf(step);
  if (!active || rows.some((h) => h.hostKey === active.hostKey)) return rows;
  return [...rows, active].sort(byName);
}

const byName = (a: NearbyHost, b: NearbyHost) =>
  a.name.toLowerCase().localeCompare(b.name.toLowerCase()) || a.hostKey.localeCompare(b.hostKey);

/** One browse's answer folded into the list on screen. A host that this
 *  browse missed stays for `keepFor` more rounds: an announcement is one
 *  multicast packet, and a row that blinks out for losing it reads as a host
 *  that left. */
export interface Sighting {
  host: NearbyHost;
  /** Browses in a row that did not hear it. */
  missed: number;
}

export function nextSightings(prev: Sighting[], found: NearbyHost[], keepFor = 2): Sighting[] {
  const heard = new Set(found.map((h) => h.hostKey));
  const kept = prev
    .filter((s) => !heard.has(s.host.hostKey) && s.missed < keepFor)
    .map((s) => ({ host: s.host, missed: s.missed + 1 }));
  return [...found.map((host) => ({ host, missed: 0 })), ...kept].sort((a, b) =>
    byName(a.host, b.host),
  );
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

/** A host's refusal as this Mac should read it. The host writes for whoever
 *  is holding the device that asked, to whom it is "your Mac"; read on another
 *  Mac that names the wrong machine, so the host is called by its name. */
export function refusalFrom(host: NearbyHost, message: string): string {
  return message.replace(/\byour Mac\b/gi, `“${host.name}”`);
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
    error: refusalFrom(host, message),
  };
}
