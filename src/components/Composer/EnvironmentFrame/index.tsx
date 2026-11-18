import type { ReactNode } from "react";
import { hostTone } from "@/remote/hostTone";
import { useHasPairedHost, useHostEntry } from "@/store/capabilities";
import { EnvironmentTab } from "./EnvironmentTab";

export { EnvironmentTab } from "./EnvironmentTab";

/** A composer in the colours of where its agent runs, for the host the calling
 *  component is scoped to: neutral on This Mac, tinted on a paired host, grey
 *  on one out of reach. With a host paired it carries a tab at its top edge
 *  naming the place: `tab` where the place can be picked, else one that only
 *  says it. A window with no paired host draws the composer as it is. */
export function EnvironmentFrame({ tab, children }: { tab?: ReactNode; children: ReactNode }) {
  const env = useHostEntry();
  const paired = useHasPairedHost();
  return (
    <div
      className="env-frame"
      data-env={env.kind}
      data-tone={env.kind === "remote" ? hostTone(env.connection, env.retrying) : undefined}
    >
      {paired && <div className="env-tab-slot">{tab ?? <EnvironmentTab env={env} />}</div>}
      {children}
    </div>
  );
}
