import { createContext, useContext } from "react";
import type { KeyedLink } from ".";

/** The host the components under it act on (`OnHost`). */
export const HostContext = createContext<KeyedLink | null>(null);

/** The link of the host this component acts on: the one the `OnHost` above it
 *  names. */
export function useHost(): KeyedLink {
  const link = useContext(HostContext);
  if (!link) throw new Error("useHost: nothing names a host here; render this under <OnHost>");
  return link;
}
