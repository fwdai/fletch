import type { AutopilotState, StuckReason } from "@/autopilot";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";

// ── Autopilot control + status, per checkout ──────────────────────────────────
// An unattended loop that spends agent turns has to be visible and stoppable
// from the surface it acts on, so this is both the switch and the readout. Off is
// the default everywhere; nothing here starts on its own.

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
  off: "Autopilot",
  idle: "Autopilot on",
  working: "Autopilot working…",
  paused: "Autopilot paused",
  stuck: "Autopilot stopped",
};

export function AutopilotChip({ agentId, subdir }: { agentId: string; subdir?: string }) {
  const key = checkoutKey(agentId, subdir);
  const state = useAppStore((s) => s.autopilot[key]);
  const enroll = useAppStore((s) => s.enrollAutopilot);
  const unenroll = useAppStore((s) => s.unenrollAutopilot);
  const pause = useAppStore((s) => s.pauseAutopilot);
  const resume = useAppStore((s) => s.resumeAutopilot);

  const mode = chipMode(state);
  const attempt = state?.cycle?.attempt;

  // Primary click: the least surprising thing for the current mode. Enrolling is
  // the only action that starts work, and it takes an explicit click every time.
  const onClick = () => {
    if (mode === "off") return enroll(key);
    if (mode === "stuck" || mode === "paused") return resume(key);
    return pause(key);
  };

  const title =
    mode === "stuck" && state?.stuck
      ? `${stuckLabel(state.stuck.reason)}. Click to let it try again.`
      : mode === "off"
        ? "Let Fletch fix failing checks on this PR without being asked"
        : mode === "paused"
          ? "Click to resume"
          : "Click to pause";

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
      {/* Turning it off entirely is deliberately separate from pausing, so
       *  "stop for now" can't be mistaken for "forget this checkout". */}
      {mode !== "off" && (
        <button
          type="button"
          className="ap-off text-xs"
          onClick={() => unenroll(key)}
          title="Turn autopilot off for this checkout"
        >
          Off
        </button>
      )}
    </div>
  );
}
