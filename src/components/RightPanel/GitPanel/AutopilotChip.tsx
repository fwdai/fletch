import type { AutopilotState, StuckReason } from "@/autopilot";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { autopilotProjectOn } from "@/store/autopilot";
import { checkoutKey } from "@/store/git";

// ── Autopilot control + status, per checkout ──────────────────────────────────
// An unattended loop that spends agent turns has to be visible and stoppable
// from the surface it acts on, so this is both the readout and the pause switch.
// Autopilot is on by default, per project: turning it OFF is a project-settings
// decision, so the chip only pauses and resumes this one checkout, and when the
// project has it off the chip says so and leads to the switch.

/** What the chip says, derived from the stored state. Kept separate from the
 *  component so the phrasing is testable and the render stays dumb. */
export type ChipMode = "off" | "idle" | "working" | "paused" | "stuck";

export function chipMode(state: AutopilotState | undefined): ChipMode {
  if (!state?.enrolled) return "off";
  if (state.stuck) return "stuck";
  if (state.paused) return "paused";
  return state.cycle ? "working" : "idle";
}

/** Why autopilot handed this checkout back — phrased as what happened, not as an
 *  error code, since the next move is the user's. */
export function stuckLabel(reason: StuckReason): string {
  switch (reason) {
    case "budget-spent":
      return "Autopilot tried three times without fixing it";
    case "no-progress":
      return "Autopilot stopped — its last attempt changed nothing";
    case "needs-human":
      return "Autopilot stopped — this needs you";
    case "disputed-review":
      return "Autopilot pushed back on a review comment — see the thread";
    case "dirty-tree":
      return "Autopilot stopped — it won't commit your uncommitted changes";
    case "no-evidence":
      return "Autopilot stopped — no result came back";
  }
}

const LABEL: Record<ChipMode, string> = {
  off: "Autopilot off",
  idle: "Autopilot on",
  working: "Autopilot working…",
  paused: "Autopilot paused",
  stuck: "Autopilot stopped",
};

export function AutopilotChip({ agentId, subdir }: { agentId: string; subdir?: string }) {
  const key = checkoutKey(agentId, subdir);
  const state = useAppStore((s) => s.autopilot[key]);
  const agent = useAppStore((s) => s.workspace?.agents.find((a) => a.id === agentId));
  // Off when the project switched it off — or when the opt-outs never loaded,
  // in which case the driver is not running and the chip must not claim it is.
  // Same predicate the driver uses, so the two can't disagree.
  const projectOff = useAppStore(
    (s) => agent == null || !autopilotProjectOn(s.autopilotDisabledProjects, agent.project_id),
  );
  const enroll = useAppStore((s) => s.enrollAutopilot);
  const pause = useAppStore((s) => s.pauseAutopilot);
  const resume = useAppStore((s) => s.resumeAutopilot);
  const openProjectScreen = useAppStore((s) => s.openProjectScreen);

  // The driver enrolls a checkout on its first tick, so an absent entry for an
  // enabled project is just "not ticked yet" — read it as on, which it is.
  const mode: ChipMode = projectOff ? "off" : state ? chipMode(state) : "idle";
  const attempt = state?.cycle?.attempt;

  // Primary click: the least surprising thing for the current mode. Off is a
  // project decision, so that click goes to where the switch lives.
  const onClick = () => {
    if (mode === "off") {
      const repoPath = agent?.repos[0]?.repo_path;
      if (repoPath) openProjectScreen(repoPath, "settings");
      return;
    }
    if (mode === "stuck" || mode === "paused") return resume(key);
    // Pausing before the first tick: create the entry so the pause has somewhere
    // to land (the store ignores transitions on an absent checkout).
    if (!state) enroll(key);
    pause(key);
  };

  const title =
    mode === "stuck" && state?.stuck
      ? `${stuckLabel(state.stuck.reason)}. Click to let it try again.`
      : mode === "off"
        ? "Autopilot is off for this project. Click to open project settings."
        : mode === "paused"
          ? "Click to resume"
          : "Fletch fixes failing checks, conflicts and review comments on this PR without being asked. Click to pause.";

  return (
    <div className="git-autopilot flex-center">
      <button type="button" className={`ap-chip text-xs m-${mode}`} onClick={onClick} title={title}>
        <Icon name={mode === "working" ? "refresh" : "wrench"} />
        <span>{LABEL[mode]}</span>
        {/* Attempt count only while it means something — a second or third try is
         *  exactly when a user wants to know before it gives up. */}
        {mode === "working" && attempt != null && attempt > 1 && (
          <span className="ap-attempt">#{attempt}</span>
        )}
      </button>
    </div>
  );
}
