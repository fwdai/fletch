import type { DotStatus } from "./derive";

/** An agent's glanceable state, as a dot: the workspace popover's head, the
 *  Home summary, the command palette. `running` pings; the ping is disabled
 *  under prefers-reduced-motion (see WorkspaceStatus.css). */
export function StatusDot({ status, big }: { status: DotStatus; big?: boolean }) {
  return <span className={`ws-dot ${status}${big ? " big" : ""}`} aria-hidden />;
}
