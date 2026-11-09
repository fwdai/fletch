// The Fletch hosts announcing themselves on this network
// (docs/remote-protocol.md, "Discovery"), as both browsers report them: the
// phone's nearby plugin and the desktop's `remote_browse_nearby`. One shape,
// so the two lists cannot drift apart.

import { localHostname } from "./pairing";
import type { HostTarget } from "./types";

/** One host as its announcement describes it. A claim, not a credential: the
 *  handshake is what proves `hostKey`. */
export interface NearbyHost {
  name: string;
  hostKey: string;
  port: number;
  /** The IPv4 addresses the announcement resolved to. The desktop's browse
   *  fills it in; the phone's plugin does not, and dials the `.local` name. */
  addresses?: string[];
}

/** Where to dial a host picked from the list, pinning the key it claims.
 *
 *  The resolved address when the browse has one, with the `.local` name the
 *  key derives raced beside it (`candidatesFor` adds it whenever the target
 *  holds a key), so whichever answers first wins and the saved record keeps
 *  both ways in. With no address, the `.local` name alone, as the phone does.
 *  Null for a key no name derives from, which no handshake could prove
 *  either. */
export function nearbyTarget(host: NearbyHost): (HostTarget & { hostKey: string }) | null {
  const local = localHostname(host.hostKey);
  if (!local) return null;
  return {
    host: host.addresses?.[0] ?? local,
    port: host.port,
    hostKey: host.hostKey,
    name: host.name,
  };
}
