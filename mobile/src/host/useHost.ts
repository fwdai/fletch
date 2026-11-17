import { createContext, useContext } from "react";
import { useApp } from "../store";
import type { HostLink } from ".";

/** The host the components under it act on. Left unset, they act on the one
 *  the app is live on. */
export const HostContext = createContext<HostLink | null>(null);

/** The link of the host this component acts on: the one a `HostContext` above
 *  it names, else the one the app is live on — the active host's wherever a
 *  host is active, and a pairing's on the Pair screen. */
export function useHost(): HostLink {
  const scoped = useContext(HostContext);
  const live = useApp((s) => s.liveLink);
  return scoped ?? live;
}
