// Module state that belongs to one engine, kept per host.
//
// Some of what the store's helpers hold outside the store is keyed by an agent
// id or a checkout (the agents a stop silenced, the rows a click hid), or
// orders reads of one engine against each other (a refresh's generation). Two
// hosts reuse agent ids and run their reads independently, so each gets its
// own copy, made on first use and dropped when the host is forgotten.

import type { EnvironmentId } from "./environments";

const tables = new Set<Map<EnvironmentId, unknown>>();

/** A per-host copy of whatever `make` builds: `of(hostId)` answers with that
 *  host's, building it the first time it is asked for. */
export function perHost<T>(make: () => T): (hostId: EnvironmentId) => T {
  const byHost = new Map<EnvironmentId, T>();
  tables.add(byHost as Map<EnvironmentId, unknown>);
  return (hostId) => {
    let value = byHost.get(hostId);
    if (value === undefined) {
      value = make();
      byHost.set(hostId, value);
    }
    return value;
  };
}

/** Drop every per-host copy `hostId` has. */
export function forgetHostState(hostId: EnvironmentId): void {
  for (const byHost of tables) byHost.delete(hostId);
}
