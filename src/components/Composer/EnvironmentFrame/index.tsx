import type { ReactNode } from "react";
import { hostTone } from "@/remote/hostTone";
import { useHasPairedHost, useHostEntry } from "@/store/capabilities";
import { EnvironmentTab } from "./EnvironmentTab";

export { EnvironmentTab } from "./EnvironmentTab";

/** A composer in the colours of where its agent runs, for the host the calling
 *  component is scoped to: neutral on This Mac, tinted on a connected paired
 *  host, grey on one out of reach (`data-tone`, which the stylesheet keys on).
 *  With a host paired a sheet sticks out from behind its top edge naming the
 *  machine: `tab` where it can be picked, else a tab that only says it. A
 *  window with no paired host draws the composer as it is. */
export function EnvironmentFrame({ tab, children }: { tab?: ReactNode; children: ReactNode }) {
  const env = useHostEntry();
  const paired = useHasPairedHost();
  return (
    <div
      className="env-frame"
      data-env={env.kind}
      data-tone={env.kind === "remote" ? hostTone(env.connection, env.retrying) : undefined}
    >
      {paired && <div className="env-sheet">{tab ?? <EnvironmentTab env={env} />}</div>}
      {children}
    </div>
  );
}
