// Where to dial, and in which order: the LAN address first with a short open
// timeout, then the relay (docs/remote-protocol.md, "Relay" → "Phone side").
// The Noise handshake and every frame after it are identical on both paths, so
// this is the only place that knows there is more than one.

import { relayDeviceUrl, wsUrl } from "./pairing";
import type { HostTarget, Via } from "./types";

/** How long to wait for the LAN socket to open before moving on. The timeout
 *  is enforced by the transport, not by a timer on this side. */
export const LAN_OPEN_TIMEOUT_MS = 3000;

export interface Candidate {
  url: string;
  via: Via;
  /** Absent means "no bound" — the relay is the last resort, so waiting on it
   *  is waiting on the only path left. */
  timeoutMs?: number;
}

/** The dial list for `target`. The relay is only reachable with the host key,
 *  which is also its route on the relay, so a hand-typed target (no key yet)
 *  is LAN-only until a pairing link supplies both. */
export function candidatesFor(target: HostTarget, lanTimeoutMs = LAN_OPEN_TIMEOUT_MS): Candidate[] {
  const list: Candidate[] = [{ url: wsUrl(target), via: "lan", timeoutMs: lanTimeoutMs }];
  if (target.relay && target.hostKey) {
    list.push({ url: relayDeviceUrl(target.relay, target.hostKey), via: "relay" });
  }
  return list;
}
