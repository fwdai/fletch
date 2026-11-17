import { createContext, useContext, useMemo } from "react";
import { type AppState, useApp } from "../store";
import { withHost } from "../store/hostNav";
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

/** The app's `push` and `openSheet`, naming in what they open the host this
 *  component acts on — so the screen or sheet acts on that host too, whichever
 *  is on screen by then. */
export function useHostNav(): Pick<AppState, "push" | "openSheet"> {
  const { hostKey } = useHost();
  const push = useApp((s) => s.push);
  const openSheet = useApp((s) => s.openSheet);
  return useMemo(
    () => ({
      push: (screen, props) => push(screen, withHost(hostKey, props)),
      openSheet: (name, props) => openSheet(name, withHost(hostKey, props)),
    }),
    [hostKey, push, openSheet],
  );
}
