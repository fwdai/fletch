import type { DotStatus } from "./derive";

/** The agent's glanceable state. In the capsule it badges the corner of the
 *  workspace's isolation icon (`.ws-mark`). `running` pings; the ping is
 *  disabled under prefers-reduced-motion (see WorkspaceStatus.css). */
export function StatusDot({ status, big }: { status: DotStatus; big?: boolean }) {
  return <span className={`ws-dot ${status}${big ? " big" : ""}`} aria-hidden />;
}
