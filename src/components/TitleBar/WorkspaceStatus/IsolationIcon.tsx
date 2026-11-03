import type { BoxStateEvent } from "@/api";
import { Icon } from "@/components/Icon";
import { BOX_PHASE_LABEL, type Isolation } from "./isolation";

/** The glyph in front of the workspace name: how the agent is confined. A
 *  box's icon also carries its phase as a tint (lit while it runs, pulsing
 *  while it comes up or goes down, red when it failed), since a box can be
 *  stopped while its workspace is still open; the popover spells it out.
 *  Seatbelt has no such lifecycle, so its icon is always lit. While the
 *  workspace moves, `isolation` is where it's going and the icon pulses. */
export function IsolationIcon({
  isolation,
  boxPhase,
  moving = false,
  size = 13,
}: {
  isolation: Isolation;
  boxPhase?: BoxStateEvent["phase"] | null;
  moving?: boolean;
  size?: number;
}) {
  const phase = !moving && isolation.kind === "box" ? boxPhase : null;
  const state = moving ? "Moving" : phase ? BOX_PHASE_LABEL[phase] : null;
  return (
    <span
      className={["ws-iso", isolation.kind, phase, moving && "moving"].filter(Boolean).join(" ")}
      role="img"
      aria-label={state ? `${isolation.title} · ${state}` : isolation.title}
    >
      <Icon name={isolation.icon} size={size} />
    </span>
  );
}
