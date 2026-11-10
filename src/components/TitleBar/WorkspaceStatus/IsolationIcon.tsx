import type { BoxStateEvent } from "@/api";
import { Icon } from "@/components/Icon";
import { BOX_PHASE_LABEL, type Isolation } from "./isolation";

/** The glyph in front of the workspace name: how the agent is confined. A
 *  box's icon also carries its phase as a tint (lit while it runs, pulsing
 *  while it comes up or goes down, red when it failed), since a box can be
 *  stopped while its workspace is still open; the popover spells it out.
 *  Seatbelt has no such lifecycle, so its icon is always lit. While the
 *  workspace moves, `isolation` is where it's going and the icon pulses.
 *  With `tip`, the label it carries for a screen reader is its tooltip too,
 *  for a surface with no popover of its own to spell it out. */
export function IsolationIcon({
  isolation,
  boxPhase,
  moving = false,
  size = 13,
  tip = false,
}: {
  isolation: Isolation;
  boxPhase?: BoxStateEvent["phase"] | null;
  moving?: boolean;
  size?: number;
  tip?: boolean;
}) {
  const phase = !moving && isolation.kind === "box" ? boxPhase : null;
  const state = moving ? "Moving" : phase ? BOX_PHASE_LABEL[phase] : null;
  const label = state ? `${isolation.title} · ${state}` : isolation.title;
  return (
    <span
      className={["ws-iso", isolation.kind, phase, moving && "moving", tip && "tip"]
        .filter(Boolean)
        .join(" ")}
      role="img"
      aria-label={label}
      data-tip={tip ? label : undefined}
    >
      <Icon name={isolation.icon} size={size} />
    </span>
  );
}
