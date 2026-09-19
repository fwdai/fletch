import { hostSkew } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import pkg from "../../package.json";

/** One line saying what a paired host cannot do, with the reasons and both
 *  versions in its tooltip. Renders nothing for the local environment, for a
 *  host that has not been greeted yet, and for a host that answers everything —
 *  which is why the two surfaces that identify a host (Settings › Remote
 *  control › Paired hosts, and the sidebar's environment switcher) can both
 *  drop it in unconditionally.
 *
 *  `className` is the caller's own sub-line class: the note is a line in an
 *  existing stack, not a component with a look of its own. */
export function HostGateNote({ env, className }: { env: EnvironmentEntry; className: string }) {
  const skew = hostSkew(env, pkg.version);
  if (!skew) return null;
  return (
    <span className={`${className} tip`} data-tip={skew.tip}>
      {skew.summary}
    </span>
  );
}
