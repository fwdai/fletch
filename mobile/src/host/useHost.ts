import { createContext, useContext, useMemo } from "react";
import { type AppState, linkOnScreen, useApp } from "../store";
import type { HostLink } from ".";

/** The host the components under it act on. Left unset, they act on the host
 *  on screen (`linkOnScreen`). */
export const HostContext = createContext<HostLink | null>(null);

/** The link of the host this component acts on: the one a `HostContext` above
 *  it names, else the one on screen. */
export function useHost(): HostLink {
  const scoped = useContext(HostContext);
  const shown = useApp(linkOnScreen);
  const link = scoped ?? shown;
  if (!link) throw new Error("useHost: no host to act on; render this under <OnHost>");
  return link;
}

/** The app's `push` and `openSheet`, naming in what they open the host this
 *  component acts on — so the screen or sheet acts on that host too, whichever
 *  is on screen by then. */
export function useHostNav(): Pick<AppState, "push" | "openSheet"> {
  const { hostKey } = useHost();
  const push = useApp((s) => s.push);
  const openSheet = useApp((s) => s.openSheet);
  return useMemo(() => {
    const named = (props: Record<string, string> = {}) => (hostKey ? { ...props, hostKey } : props);
    return {
      push: (screen, props) => push(screen, named(props)),
      openSheet: (name, props) => openSheet(name, named(props)),
    };
  }, [hostKey, push, openSheet]);
}
