import type { DotStatus } from "./derive";

/** The single glanceable signal at the front of the capsule. `running` pulses;
 *  the ping is disabled under prefers-reduced-motion (see TitleBar.css). */
export function StatusDot({ status, big }: { status: DotStatus; big?: boolean }) {
  return <span className={`ws-dot ${status}${big ? " big" : ""}`} aria-hidden />;
}
