import { createContext, useContext } from "react";
import { useStore } from "../store";
import { type HostLink, linkOf } from ".";

/** The host the components under it act on. Left unset, they act on the
 *  active one. */
export const HostContext = createContext<HostLink | null>(null);

/** The link of the host this component acts on: the one a `HostContext` above
 *  it names, else the active host's. Only for what is on screen while a host
 *  is active — which is everything but the Pair screen. */
export function useHost(): HostLink {
  const scoped = useContext(HostContext);
  const hostKey = useStore((s) => s.hostKey);
  if (scoped) return scoped;
  if (!hostKey) throw new Error("No host is active.");
  return linkOf(hostKey);
}
