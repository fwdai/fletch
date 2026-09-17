import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { autopilotProjectOn } from "@/store/autopilot";

// ── The kill switch ──────────────────────────────────────────────────────────
// Autopilot is on by default and acts without being asked, which is right until
// someone (or another agent) is already on the PR — then two hands on the same
// branch is worse than none. This is the per-workspace way out: one glyph in
// the status header, beside the GitHub link, where it is always reachable —
// including BEFORE autopilot has done anything, which is exactly when you want
// to stop it. The zap is autopilot's glyph everywhere in the app (the sidebar's
// working mark, the "started automatically" mark in the status line), so lit
// means on and struck means off without a label.
//
// Scoped to the agent, not the checkout: every repo of a multi-repo workspace
// pauses together, so a multi-repo panel shows this once.

/** Hover copy: one short line, doubling as the switch's accessible name. */
function switchTip(projectOn: boolean, on: boolean): string {
  if (!projectOn) return "Autopilot is off for this project";
  return on
    ? "Autopilot on · click to pause for this workspace"
    : "Autopilot paused · click to resume";
}

export function AutopilotSwitch({ agentId, projectId }: { agentId: string; projectId: string }) {
  const disabledProjects = useAppStore((s) => s.autopilotDisabledProjects);
  const paused = useAppStore((s) => s.autopilotPausedAgents.includes(agentId));
  const setAgentAutopilot = useAppStore((s) => s.setAgentAutopilot);

  // With the project switch off there is nothing here to pause: show the glyph
  // struck and inert rather than hide it, so the answer to "where did autopilot
  // go" is on screen.
  const projectOn = autopilotProjectOn(disabledProjects, projectId);
  const on = projectOn && !paused;
  const tip = switchTip(projectOn, on);

  return (
    <button
      type="button"
      className={`hdr-auto tip ${on ? "on" : "off"}`}
      role="switch"
      aria-checked={on}
      aria-label={tip}
      data-tip={tip}
      disabled={!projectOn}
      onClick={() => setAgentAutopilot(agentId, !on)}
    >
      <Icon name={on ? "zap" : "zapOff"} size={13} />
    </button>
  );
}
