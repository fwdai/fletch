import type { BoxStateEvent } from "@/api";
import { Icon } from "@/components/Icon";
import { BOX_PHASE_LABEL, type Isolation } from "./isolation";

/** The glyph in front of the workspace name: how the agent is confined. A
 *  box's icon also carries its phase as a tint (lit while it runs, pulsing
 *  while it comes up or goes down, red when it failed), since a box can be
 *  stopped while its workspace is still open; the popover spells it out.
 *  Seatbelt has no such lifecycle, so its icon is always lit. */
export function IsolationIcon({
  isolation,
  boxPhase,
  size = 13,
}: {
  isolation: Isolation;
  boxPhase?: BoxStateEvent["phase"] | null;
  size?: number;
}) {
  const phase = isolation.kind === "box" ? boxPhase : null;
  const label = phase ? `${isolation.title} · ${BOX_PHASE_LABEL[phase]}` : isolation.title;
  return (
    <span
      className={["ws-iso", isolation.kind, phase].filter(Boolean).join(" ")}
      role="img"
      aria-label={label}
    >
      <Icon name={isolation.icon} size={size} />
    </span>
  );
}
