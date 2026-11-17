// How a slice reaches a host other than its own. A slice drives its own host
// through the `get` / `set` it was built over and cannot import the store it
// is part of, so store/index.ts hands these in once, the way it hands its
// getter to ./environments. The one slice that needs them moves a draft to the
// host it is to run on (./moveDraft).

import type { EnvironmentId } from "./environments";
import type { HostStore } from "./hostScope";
import type { AppState } from "./types";

export interface OtherHosts {
  /** `hostId`'s `getState` / `setState`, its actions bound to its engine. */
  hostStore: (hostId: EnvironmentId) => HostStore;
  /** Put `hostId` on screen, then run `act` with its state and actions. */
  onHost: (hostId: EnvironmentId, act?: (s: AppState) => void) => void;
}

let source: OtherHosts | null = null;

export function setOtherHosts(hosts: OtherHosts): void {
  source = hosts;
}

export function otherHosts(): OtherHosts {
  if (!source) throw new Error("The store has not been built yet.");
  return source;
}
