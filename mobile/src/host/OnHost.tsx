import type { ReactNode } from "react";
import { linkOf } from ".";
import { HostContext } from "./useHost";

/** `children` acting on `hostKey`'s host wherever they reach for one
 *  (`useHost`, `useHostStore`), whichever host is on screen — or, with none,
 *  on the one on screen. What a screen or sheet is rendered under, from the
 *  host its props name. Always the same element, so a sheet whose props empty
 *  as it closes keeps its closing animation. */
export function OnHost({ hostKey, children }: { hostKey?: string; children?: ReactNode }) {
  return (
    <HostContext.Provider value={hostKey ? linkOf(hostKey) : null}>{children}</HostContext.Provider>
  );
}
